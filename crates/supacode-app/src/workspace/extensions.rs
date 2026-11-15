//! Onglet Skills : skills installés (activés ou non), bibliothèque (SkillsMP et skills
//! officiels d'Anthropic, ou n'importe quel dossier GitHub) et serveurs MCP.
//! Les désactivations s'appliquent aux sessions lancées par SupaCode (`--settings`).

use std::collections::HashSet;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use supacode_core::audit::{self, kind};
use supacode_core::extensions::{self, MarketSkill, McpServer, Scope, Settings, Skill};
use supacode_core::plugins::{self, Catalog, Outcome};

use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum ExtTab {
    Skills,
    Plugins,
    Library,
    Mcp,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Source {
    SkillsMp,
    Anthropic,
    Plugins,
}

pub(super) struct Extensions {
    pub settings: Settings,
    pub tab: ExtTab,
    pub source: Source,
    pub skills: Vec<Skill>,
    pub mcp: Vec<McpServer>,
    pub results: Option<Result<Vec<MarketSkill>, String>>,
    pub searching: bool,
    /// Adresses en cours d'installation.
    pub installing: HashSet<String>,
    pub query: Entity<InputState>,
    pub link: Entity<InputState>,
    pub key: Entity<InputState>,
    /// Plugins de Claude Code (installés et disponibles), chargés à la demande.
    pub catalog: Option<Result<Catalog, String>>,
    pub loading_catalog: bool,
    /// Filtre de la bibliothèque de plugins (texte de recherche validé).
    pub plugin_filter: String,
    /// Plugins en cours d'installation ou de modification.
    pub plugin_busy: HashSet<String>,
}

impl Extensions {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let settings = extensions::load_settings();
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Chercher un skill (pdf, tests, sécurité…)"));
        let link = cx.new(|cx| InputState::new(window, cx).placeholder("https://github.com/…/tree/main/skills/mon-skill"));
        let key_value = settings.skillsmp_key.clone().unwrap_or_default();
        let key = cx.new(|cx| InputState::new(window, cx).placeholder("sk_live_… (facultative)").default_value(key_value));
        cx.subscribe(&query, |this: &mut Workspace, _, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.search_skills(cx);
            }
        })
        .detach();
        cx.subscribe(&key, |this: &mut Workspace, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let value = input.read(cx).value().trim().to_owned();
                this.extensions.settings.skillsmp_key = (!value.is_empty()).then_some(value);
                let _ = extensions::save_settings(&this.extensions.settings);
            }
        })
        .detach();
        Self { settings, tab: ExtTab::Skills, source: Source::SkillsMp, skills: Vec::new(), mcp: Vec::new(), results: None, searching: false, installing: HashSet::new(), query, link, key, catalog: None, loading_catalog: false, plugin_filter: String::new(), plugin_busy: HashSet::new() }
    }
}

impl Workspace {
    pub(super) fn refresh_extensions(&mut self, cx: &mut Context<Self>) {
        let root = self.current_project().map(|p| p.path);
        self.extensions.skills = extensions::list_skills(root.as_deref(), &self.extensions.settings);
        self.extensions.mcp = extensions::list_mcp(root.as_deref(), &self.extensions.settings);
        cx.notify();
    }

    /// Réglages `--settings` des extensions désactivées, pour un projet.
    pub(super) fn extension_settings(&self, root: &Path) -> Option<serde_json::Value> {
        self.extensions.settings.claude_settings(&extensions::project_servers(root))
    }

    pub(super) fn browser_tools_enabled(&self) -> bool {
        !self.extensions.settings.mcp_off.iter().any(|s| s == extensions::SUPACODE_SERVER)
    }

    fn note(&self, summary: String) {
        if let Some(project) = self.current_project() {
            audit::record(&project.path, kind::POLICY, summary, serde_json::Value::Null, None);
        }
    }

    fn toggle_skill(&mut self, name: String, cx: &mut Context<Self>) {
        let off = &mut self.extensions.settings.skills_off;
        let enabled = off.contains(&name);
        if enabled {
            off.retain(|s| *s != name);
        } else {
            off.push(name.clone());
        }
        let _ = extensions::save_settings(&self.extensions.settings);
        self.note(format!("Skill « {name} » {}", if enabled { "activé" } else { "désactivé" }));
        self.refresh_extensions(cx);
    }

    fn toggle_mcp(&mut self, name: String, cx: &mut Context<Self>) {
        let off = &mut self.extensions.settings.mcp_off;
        let enabled = off.contains(&name);
        if enabled {
            off.retain(|s| *s != name);
        } else {
            off.push(name.clone());
        }
        let _ = extensions::save_settings(&self.extensions.settings);
        self.note(format!("Serveur MCP « {name} » {}", if enabled { "activé" } else { "désactivé" }));
        self.refresh_extensions(cx);
    }

    pub(super) fn search_skills(&mut self, cx: &mut Context<Self>) {
        let query = self.extensions.query.read(cx).value().trim().to_owned();
        let source = self.extensions.source;
        if source == Source::Plugins {
            self.extensions.plugin_filter = query;
            if self.extensions.catalog.is_none() {
                self.load_plugins(cx);
            }
            cx.notify();
            return;
        }
        if source == Source::SkillsMp && query.is_empty() {
            self.extensions.results = Some(Err("Tape un mot-clé : SkillsMP ne propose que la recherche.".into()));
            cx.notify();
            return;
        }
        let key = self.extensions.settings.skillsmp_key.clone();
        self.extensions.searching = true;
        cx.notify();
        let task = cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    match source {
                        Source::SkillsMp => extensions::search_skillsmp(&query, key.as_deref()),
                        Source::Anthropic => extensions::anthropic_catalog().map(|all| {
                            let q = query.to_lowercase();
                            all.into_iter().filter(|s| q.is_empty() || s.name.to_lowercase().contains(&q) || s.description.to_lowercase().contains(&q)).collect()
                        }),
                        Source::Plugins => Ok(Vec::new()),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.extensions.searching = false;
                this.extensions.results = Some(found.map_err(|e| format!("{e:#}")));
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    /// Dossier où lancer `claude plugin` : le projet (portée projet), sinon le dossier personnel.
    fn plugin_cwd(&self) -> PathBuf {
        self.current_project().map(|p| p.path).or_else(dirs::home_dir).unwrap_or_else(std::env::temp_dir)
    }

    pub(super) fn load_plugins(&mut self, cx: &mut Context<Self>) {
        if self.extensions.loading_catalog {
            return;
        }
        self.extensions.loading_catalog = true;
        cx.notify();
        let cwd = self.plugin_cwd();
        let task = cx.spawn(async move |this, cx| {
            let catalog = cx.background_executor().spawn(async move { plugins::catalog(&cwd) }).await;
            let _ = this.update(cx, |this, cx| {
                this.extensions.loading_catalog = false;
                this.extensions.catalog = Some(catalog.map_err(|e| format!("{e:#}")));
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    /// Installe un plugin ; si sa marketplace veut lancer une commande, elle est montrée
    /// et n'est acceptée qu'après confirmation.
    fn install_plugin(&mut self, id: String, scope: Scope, accept: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if scope == Scope::Project && self.current_project().is_none() {
            self.toast("Ajoute un projet pour y installer un plugin.", cx);
            return;
        }
        if accept.is_none() && !self.extensions.plugin_busy.insert(id.clone()) {
            return;
        }
        cx.notify();
        let cwd = self.plugin_cwd();
        let task = cx.spawn_in(window, async move |this, cx| {
            let (job_id, job_accept) = (id.clone(), accept.clone());
            let result = cx.background_executor().spawn(async move { plugins::install(&job_id, scope, &cwd, job_accept.as_deref()) }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(Outcome::Done(_)) => {
                        this.extensions.plugin_busy.remove(&id);
                        this.note(format!("Plugin « {id} » installé ({})", scope.label()));
                        this.toast(format!("Plugin « {id} » installé : actif dans les prochaines sessions de Claude."), cx);
                        this.extensions.catalog = None;
                        this.load_plugins(cx);
                    }
                    Ok(Outcome::NeedsConfirmation(confirmation)) => {
                        let detail = format!("Sa marketplace veut lancer cette commande sur ta machine :

{}

N'accepte que si tu fais confiance à cette source.", confirmation.command);
                        let answer = window.prompt(PromptLevel::Warning, &format!("Installer « {id} » ?"), Some(&detail), &["Lancer la commande et installer", "Annuler"], cx);
                        let task = cx.spawn_in(window, async move |this, cx| {
                            let accepted = answer.await == Ok(0);
                            let _ = this.update_in(cx, |this, window, cx| {
                                if accepted {
                                    this.install_plugin(id.clone(), scope, Some(confirmation.sha256.clone()), window, cx);
                                } else {
                                    this.extensions.plugin_busy.remove(&id);
                                    cx.notify();
                                }
                            });
                        });
                        this._tasks.push(task);
                    }
                    Err(e) => {
                        this.extensions.plugin_busy.remove(&id);
                        this.toast(format!("Installation impossible : {e:#}"), cx);
                    }
                }
            });
        });
        self._tasks.push(task);
    }

    fn toggle_plugin(&mut self, id: String, scope: String, enabled: bool, cx: &mut Context<Self>) {
        if !self.extensions.plugin_busy.insert(id.clone()) {
            return;
        }
        cx.notify();
        let cwd = self.plugin_cwd();
        let task = cx.spawn(async move |this, cx| {
            let (job_id, job_scope) = (id.clone(), scope.clone());
            let result = cx.background_executor().spawn(async move { plugins::set_enabled(&job_id, &job_scope, !enabled, &cwd) }).await;
            let _ = this.update(cx, |this, cx| {
                this.extensions.plugin_busy.remove(&id);
                match result {
                    Ok(()) => {
                        this.note(format!("Plugin « {id} » {}", if enabled { "désactivé" } else { "activé" }));
                        if let Some(Ok(catalog)) = &mut this.extensions.catalog {
                            if let Some(p) = catalog.installed.iter_mut().find(|p| p.id == id) {
                                p.enabled = !enabled;
                            }
                        }
                    }
                    Err(e) => this.toast(format!("{e:#}"), cx),
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    fn uninstall_plugin(&mut self, id: String, scope: String, window: &mut Window, cx: &mut Context<Self>) {
        let answer = window.prompt(PromptLevel::Warning, &format!("Désinstaller « {id} » ?"), Some(&format!("Le plugin est retiré de Claude Code (portée « {scope} »).")), &["Désinstaller", "Annuler"], cx);
        let cwd = self.plugin_cwd();
        let task = cx.spawn(async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let (job_id, job_scope) = (id.clone(), scope.clone());
            let result = cx.background_executor().spawn(async move { plugins::uninstall(&job_id, &job_scope, &cwd) }).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.note(format!("Plugin « {id} » désinstallé")),
                    Err(e) => this.toast(format!("{e:#}"), cx),
                }
                this.extensions.catalog = None;
                this.load_plugins(cx);
            });
        });
        self._tasks.push(task);
    }

    fn install_skill(&mut self, url: String, scope: Scope, cx: &mut Context<Self>) {
        let root = self.current_project().map(|p| p.path);
        let Some(dest) = extensions::skills_dir(root.as_deref(), scope) else {
            self.toast("Ajoute un projet pour y installer un skill.", cx);
            return;
        };
        if !self.extensions.installing.insert(url.clone()) {
            return;
        }
        cx.notify();
        let task = cx.spawn(async move |this, cx| {
            let job_url = url.clone();
            let result = cx.background_executor().spawn(async move { extensions::install(&job_url, &dest) }).await;
            let _ = this.update(cx, |this, cx| {
                this.extensions.installing.remove(&url);
                match result {
                    Ok(dir) => {
                        this.extensions.settings.installed.insert(dir.display().to_string(), url.clone());
                        let _ = extensions::save_settings(&this.extensions.settings);
                        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        this.note(format!("Skill « {name} » installé ({}) depuis {url}", scope.label()));
                        this.toast(format!("Skill « {name} » installé : Claude l'utilisera dans les prochaines conversations."), cx);
                    }
                    Err(e) => this.toast(format!("Installation impossible : {e:#}"), cx),
                }
                this.refresh_extensions(cx);
            });
        });
        self._tasks.push(task);
    }

    fn install_link(&mut self, scope: Scope, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.extensions.link.read(cx).value().trim().to_owned();
        if extensions::parse_github_url(&url).is_none() {
            self.toast("Colle l'adresse GitHub d'un dossier qui contient un SKILL.md.", cx);
            return;
        }
        self.extensions.link.update(cx, |l, cx| l.set_value("", window, cx));
        self.install_skill(url, scope, cx);
    }

    /// Supprime un skill installé par SupaCode, après confirmation.
    fn uninstall_skill(&mut self, skill: Skill, window: &mut Window, cx: &mut Context<Self>) {
        let answer = window.prompt(PromptLevel::Warning, &format!("Supprimer le skill « {} » ?", skill.name), Some(&format!("Le dossier {} sera supprimé.", skill.dir.display())), &["Supprimer", "Annuler"], cx);
        let task = cx.spawn(async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let removed = std::fs::remove_dir_all(&skill.dir);
            let _ = this.update(cx, |this, cx| {
                match removed {
                    Ok(()) => {
                        this.extensions.settings.installed.remove(&skill.dir.display().to_string());
                        this.extensions.settings.skills_off.retain(|s| *s != skill.name);
                        let _ = extensions::save_settings(&this.extensions.settings);
                        this.note(format!("Skill « {} » supprimé", skill.name));
                    }
                    Err(e) => this.toast(format!("Suppression impossible : {e}"), cx),
                }
                this.refresh_extensions(cx);
            });
        });
        self._tasks.push(task);
    }

    // — Rendu

    pub(super) fn render_extensions(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.extensions.tab;
        let segment = |id: &'static str, label: String, value: ExtTab| {
            let selected = tab == value;
            div()
                .id(id)
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.0))
                .text_size(px(12.0))
                .cursor_pointer()
                .when(selected, |d| d.bg(T.raised).text_color(T.text).font_weight(FontWeight::MEDIUM))
                .when(!selected, |d| d.text_color(T.text2).hover(|d| d.text_color(T.text)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.extensions.tab = value;
                    if value == ExtTab::Plugins && this.extensions.catalog.is_none() {
                        this.load_plugins(cx);
                    }
                    this.refresh_extensions(cx);
                }))
                .child(label)
        };
        let enabled_skills = self.extensions.skills.iter().filter(|s| s.enabled).count();
        let segments = div()
            .mx_2()
            .mb_2()
            .h(px(28.0))
            .p(px(2.0))
            .flex()
            .gap(px(2.0))
            .rounded(px(7.0))
            .bg(T.app)
            .border_1()
            .border_color(T.border)
            .child(segment("ext-skills", format!("Skills {enabled_skills}"), ExtTab::Skills))
            .child(segment("ext-plugins", format!("Plugins {}", self.extensions.catalog.as_ref().and_then(|c| c.as_ref().ok()).map(|c| c.installed.iter().filter(|p| p.enabled).count().to_string()).unwrap_or_default()), ExtTab::Plugins))
            .child(segment("ext-library", "Bibliothèque".into(), ExtTab::Library))
            .child(segment("ext-mcp", format!("MCP {}", self.extensions.mcp.iter().filter(|s| s.enabled).count()), ExtTab::Mcp));
        let body = match tab {
            ExtTab::Skills => self.render_installed(cx).into_any_element(),
            ExtTab::Library => self.render_library(cx).into_any_element(),
            ExtTab::Mcp => self.render_mcp(cx).into_any_element(),
            ExtTab::Plugins => self.render_installed_plugins(cx).into_any_element(),
        };
        div().flex_1().min_h_0().flex().flex_col().pt_1().child(segments).child(body)
    }

    fn render_installed(&mut self, cx: &mut Context<Self>) -> Stateful<Div> {
        let mut list = div().id("skill-list").flex_1().min_h_0().overflow_y_scroll().px_2().flex().flex_col().gap(px(2.0));
        if self.extensions.skills.is_empty() {
            list = list.child(empty_card(
                "Aucun skill",
                "Un skill apprend un savoir-faire à l'agent (PDF, tests, conventions de l'équipe…). Ajoute-en depuis la bibliothèque ; ceux du projet (.claude/skills) sont partagés avec l'équipe par git.",
            ));
        }
        for (i, skill) in self.extensions.skills.clone().into_iter().enumerate() {
            let (name, removable) = (skill.name.clone(), skill.source.is_some());
            let removed = skill.clone();
            list = list.child(
                div()
                    .id(("skill", i))
                    .px_1p5()
                    .py(px(7.0))
                    .rounded(px(6.0))
                    .flex()
                    .items_start()
                    .gap_2()
                    .hover(|d| d.bg(T.hover))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .child(div().truncate().font_weight(FontWeight::MEDIUM).text_color(if skill.enabled { T.text } else { T.text3 }).child(skill.name.clone()))
                                    .child(tag(skill.scope.label().into())),
                            )
                            .child(clamp(skill.description.clone(), T.text3))
                            .when_some(skill.source.clone(), |d, source| d.child(div().truncate().text_size(px(10.0)).text_color(T.text3).font_family(theme::MONO_FONT).child(source.replace("https://github.com/", "")))),
                    )
                    .when(removable, |d| {
                        d.child(
                            div()
                                .id(("skill-remove", i))
                                .size(px(20.0))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.0))
                                .text_color(T.text3)
                                .cursor_pointer()
                                .hover(|d| d.bg(T.raised).text_color(T.err))
                                .on_click(cx.listener(move |this, _, window, cx| this.uninstall_skill(removed.clone(), window, cx)))
                                .child(icon(icons::CLOSE).size(px(11.0))),
                        )
                    })
                    .child(switch(("skill-toggle", i), skill.enabled).on_click(cx.listener(move |this, _, _, cx| this.toggle_skill(name.clone(), cx)))),
            );
        }
        list.child(div().px_1p5().pt_2().pb_3().text_size(px(11.0)).text_color(T.text3).child("Désactivé : les conversations et workflows lancés par SupaCode ne l'utilisent pas. Le dossier reste en place."))
    }

    fn render_library(&mut self, cx: &mut Context<Self>) -> Stateful<Div> {
        let source = self.extensions.source;
        let chip = |id: &'static str, label: &'static str, value: Source| {
            let selected = source == value;
            div()
                .id(id)
                .h(px(24.0))
                .px_2()
                .flex()
                .items_center()
                .rounded(px(12.0))
                .text_size(px(12.0))
                .cursor_pointer()
                .when(selected, |d| d.bg(T.accent_bg).text_color(T.accent_on))
                .when(!selected, |d| d.border_1().border_color(T.border).text_color(T.text2).hover(|d| d.bg(T.hover)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.extensions.source = value;
                    this.extensions.results = None;
                    if value == Source::Anthropic {
                        this.search_skills(cx);
                    }
                    if value == Source::Plugins && this.extensions.catalog.is_none() {
                        this.load_plugins(cx);
                    }
                    cx.notify();
                }))
                .child(label)
        };
        let installed: HashSet<String> = self.extensions.skills.iter().map(|s| s.dir.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()).collect();
        let mut list = div().id("library").flex_1().min_h_0().overflow_y_scroll().px_2().flex().flex_col().gap_2();
        list = list
            .child(div().flex().gap_1p5().child(chip("source-skillsmp", "SkillsMP", Source::SkillsMp)).child(chip("source-anthropic", "Anthropic (officiel)", Source::Anthropic)).child(chip("source-plugins", "Plugins", Source::Plugins)))
            .child(
                div()
                    .h(px(30.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(T.border)
                    .bg(T.app)
                    .child(icon(icons::SEARCH).size(px(12.0)).text_color(T.text3))
                    .child(div().flex_1().min_w_0().child(Input::new(&self.extensions.query).appearance(false))),
            );
        if self.extensions.searching {
            list = list.child(div().px_1().text_size(px(12.0)).text_color(T.accent).child("Recherche…"));
        }
        if source == Source::Plugins {
            list = self.render_plugin_results(list, cx);
        } else {
        match &self.extensions.results {
            None => {
                list = list.child(div().px_1().text_size(px(12.0)).text_color(T.text3).child(match source {
                    Source::SkillsMp => "Plus de 100 000 skills de la communauté (skillsmp.com). Entrée pour chercher.",
                    Source::Anthropic => "Les skills publiés par Anthropic (github.com/anthropics/skills).",
                    Source::Plugins => "",
                }));
            }
            Some(Err(error)) => list = list.child(div().px_1().text_size(px(12.0)).text_color(T.err).child(error.clone())),
            Some(Ok(results)) if results.is_empty() => list = list.child(div().px_1().text_size(px(12.0)).text_color(T.text3).child("Aucun résultat.")),
            Some(Ok(results)) => {
                for (i, skill) in results.clone().into_iter().enumerate() {
                    let folder = extensions::parse_github_url(&skill.github_url).map(|p| p.3.rsplit('/').next().unwrap_or("").to_lowercase()).unwrap_or_default();
                    let is_installed = installed.contains(&folder);
                    let busy = self.extensions.installing.contains(&skill.github_url);
                    let (user_url, project_url) = (skill.github_url.clone(), skill.github_url.clone());
                    let meta = match skill.stars {
                        Some(stars) => format!("{} · ★ {}", skill.author, compact(stars)),
                        None => skill.author.clone(),
                    };
                    list = list.child(
                        div()
                            .id(("market", i))
                            .p_2()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(T.border)
                            .bg(T.surface)
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().flex().items_center().gap_1p5().child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(skill.name.clone())).child(div().flex_shrink_0().text_size(px(11.0)).text_color(T.text3).child(meta)))
                            .child(clamp(skill.description.clone(), T.text2))
                            .child(div().flex().items_center().gap_1p5().pt_1().map(|d| {
                                if is_installed {
                                    d.child(div().text_size(px(11.0)).text_color(T.ok).child("✓ installé"))
                                } else if busy {
                                    d.child(div().text_size(px(11.0)).text_color(T.accent).child("Installation…"))
                                } else {
                                    d.child(small_button(("install-user", i), "Pour moi").on_click(cx.listener(move |this, _, _, cx| this.install_skill(user_url.clone(), Scope::User, cx))))
                                        .child(small_button(("install-project", i), "Pour le projet").on_click(cx.listener(move |this, _, _, cx| this.install_skill(project_url.clone(), Scope::Project, cx))))
                                }
                            })),
                    );
                }
            }
        }
        }
        list.child(
            div()
                .mt_1()
                .pt_2()
                .border_t_1()
                .border_color(T.border)
                .flex()
                .flex_col()
                .gap_1p5()
                .child(section_label("Depuis un lien GitHub").px_1())
                .child(div().h(px(28.0)).px_2().flex().items_center().rounded(px(6.0)).border_1().border_color(T.border).bg(T.app).text_size(px(12.0)).child(Input::new(&self.extensions.link).appearance(false)))
                .child(
                    div()
                        .flex()
                        .gap_1p5()
                        .child(small_button(("link-user", 0), "Pour moi").on_click(cx.listener(|this, _, window, cx| this.install_link(Scope::User, window, cx))))
                        .child(small_button(("link-project", 0), "Pour le projet").on_click(cx.listener(|this, _, window, cx| this.install_link(Scope::Project, window, cx)))),
                )
                .child(section_label("Clé API SkillsMP").px_1().pt_1())
                .child(div().h(px(28.0)).px_2().flex().items_center().rounded(px(6.0)).border_1().border_color(T.border).bg(T.app).text_size(px(12.0)).child(Input::new(&self.extensions.key).appearance(false)))
                .child(div().px_1().pb_3().text_size(px(11.0)).text_color(T.text3).child("Sans clé : 50 recherches par jour. « Pour le projet » installe dans .claude/skills, à committer pour l'équipe. Un skill peut lancer des scripts : n'installe que des sources de confiance.")),
        )
    }

    fn render_mcp(&mut self, cx: &mut Context<Self>) -> Stateful<Div> {
        let mut list = div().id("mcp-list").flex_1().min_h_0().overflow_y_scroll().px_2().flex().flex_col().gap(px(2.0));
        for (i, server) in self.extensions.mcp.clone().into_iter().enumerate() {
            let name = server.name.clone();
            list = list.child(
                div()
                    .id(("mcp", i))
                    .px_1p5()
                    .py(px(7.0))
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .hover(|d| d.bg(T.hover))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(div().flex().items_center().gap_1p5().child(div().truncate().font_weight(FontWeight::MEDIUM).text_color(if server.enabled { T.text } else { T.text3 }).child(server.name.clone())).child(tag(server.scope.into())))
                            .child(div().truncate().text_size(px(11.0)).text_color(T.text3).font_family(theme::MONO_FONT).child(server.detail.clone())),
                    )
                    .child(switch(("mcp-toggle", i), server.enabled).on_click(cx.listener(move |this, _, _, cx| this.toggle_mcp(name.clone(), cx)))),
            );
        }
        list.child(div().px_1p5().pt_2().pb_3().text_size(px(11.0)).text_color(T.text3).child("Désactivé : ses outils sont interdits aux sessions lancées par SupaCode. Pour ajouter un serveur : claude mcp add, ou .mcp.json dans le projet."))
    }
}

impl Workspace {
    fn render_plugin_results(&self, mut list: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        let catalog = match &self.extensions.catalog {
            None => return list.child(div().px_1().text_size(px(12.0)).text_color(T.accent).child("Chargement des marketplaces…")),
            Some(Err(error)) => return list.child(div().px_1().text_size(px(12.0)).text_color(T.err).child(error.clone())),
            Some(Ok(catalog)) => catalog,
        };
        let filter = self.extensions.plugin_filter.to_lowercase();
        let matching: Vec<_> = catalog.available.iter().filter(|p| filter.is_empty() || p.name.to_lowercase().contains(&filter) || p.description.to_lowercase().contains(&filter)).collect();
        list = list.child(div().px_1().text_size(px(12.0)).text_color(T.text3).child(format!(
            "{} plugin(s){} dans tes marketplaces (claude plugin marketplace add pour en ajouter). Un plugin réunit skills, commandes, agents et serveurs MCP.",
            matching.len(),
            if filter.is_empty() { String::new() } else { format!(" pour « {} »", self.extensions.plugin_filter) }
        )));
        for (i, plugin) in matching.into_iter().take(40).enumerate() {
            let installed = catalog.installed.iter().any(|p| p.id == plugin.id);
            let busy = self.extensions.plugin_busy.contains(&plugin.id);
            let (user_id, project_id) = (plugin.id.clone(), plugin.id.clone());
            let meta = match plugin.installs {
                Some(n) => format!("{} · {} installations", plugin.marketplace, compact(n)),
                None => plugin.marketplace.clone(),
            };
            list = list.child(
                div()
                    .id(("plugin-market", i))
                    .p_2()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(T.border)
                    .bg(T.surface)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().truncate().font_weight(FontWeight::MEDIUM).child(plugin.name.clone()))
                    .child(div().truncate().text_size(px(11.0)).text_color(T.text3).child(meta))
                    .child(clamp(plugin.description.clone(), T.text2))
                    .child(div().flex().items_center().gap_1p5().pt_1().map(|d| {
                        if installed {
                            d.child(div().text_size(px(11.0)).text_color(T.ok).child("✓ installé"))
                        } else if busy {
                            d.child(div().text_size(px(11.0)).text_color(T.accent).child("Installation…"))
                        } else {
                            d.child(small_button(("plugin-user", i), "Pour moi").on_click(cx.listener(move |this, _, window, cx| this.install_plugin(user_id.clone(), Scope::User, None, window, cx))))
                                .child(small_button(("plugin-project", i), "Pour le projet").on_click(cx.listener(move |this, _, window, cx| this.install_plugin(project_id.clone(), Scope::Project, None, window, cx))))
                        }
                    })),
            );
        }
        list
    }

    fn render_installed_plugins(&mut self, cx: &mut Context<Self>) -> Stateful<Div> {
        let mut list = div().id("plugin-list").flex_1().min_h_0().overflow_y_scroll().px_2().flex().flex_col().gap(px(2.0));
        let installed = match &self.extensions.catalog {
            None => return list.child(div().px_1p5().text_size(px(12.0)).text_color(T.accent).child("Chargement…")),
            Some(Err(error)) => return list.child(div().px_1p5().text_size(px(12.0)).text_color(T.err).child(error.clone())),
            Some(Ok(catalog)) => catalog.installed.clone(),
        };
        if installed.is_empty() {
            list = list.child(empty_card("Aucun plugin", "Les plugins réunissent skills, commandes, agents et serveurs MCP. Parcours-les dans la bibliothèque (source « Plugins »)."));
        }
        for (i, plugin) in installed.into_iter().enumerate() {
            let busy = self.extensions.plugin_busy.contains(&plugin.id);
            let (toggle_id, toggle_scope, enabled) = (plugin.id.clone(), plugin.scope.clone(), plugin.enabled);
            let (remove_id, remove_scope) = (plugin.id.clone(), plugin.scope.clone());
            let (name, marketplace) = plugin.id.split_once('@').map(|(n, m)| (n.to_owned(), m.to_owned())).unwrap_or((plugin.id.clone(), String::new()));
            list = list.child(
                div()
                    .id(("plugin", i))
                    .px_1p5()
                    .py(px(7.0))
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .hover(|d| d.bg(T.hover))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(div().flex().items_center().gap_1p5().child(div().truncate().font_weight(FontWeight::MEDIUM).text_color(if enabled { T.text } else { T.text3 }).child(name)).child(tag(plugin.scope.clone())))
                            .child(div().truncate().text_size(px(11.0)).text_color(T.text3).font_family(theme::MONO_FONT).child(format!("{marketplace} · {}", plugin.version))),
                    )
                    .child(
                        div()
                            .id(("plugin-remove", i))
                            .size(px(20.0))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .text_color(T.text3)
                            .cursor_pointer()
                            .hover(|d| d.bg(T.raised).text_color(T.err))
                            .on_click(cx.listener(move |this, _, window, cx| this.uninstall_plugin(remove_id.clone(), remove_scope.clone(), window, cx)))
                            .child(icon(icons::CLOSE).size(px(11.0))),
                    )
                    .child(switch(("plugin-toggle", i), enabled).when(busy, |d| d.opacity(0.5)).on_click(cx.listener(move |this, _, _, cx| this.toggle_plugin(toggle_id.clone(), toggle_scope.clone(), enabled, cx)))),
            );
        }
        list.child(div().px_1p5().pt_2().pb_3().text_size(px(11.0)).text_color(T.text3).child("Ces interrupteurs règlent Claude Code lui-même (comme /plugin) : ils valent aussi hors de SupaCode."))
    }
}

fn empty_card(title: &'static str, text: &'static str) -> Div {
    div()
        .m_1()
        .p_3()
        .border_1()
        .border_dashed()
        .border_color(T.border_strong)
        .rounded(px(6.0))
        .flex()
        .flex_col()
        .gap_1p5()
        .text_size(px(12.0))
        .text_color(T.text2)
        .child(div().font_weight(FontWeight::MEDIUM).text_color(T.text).child(title))
        .child(text)
}

/// Texte limité à deux lignes environ.
fn clamp(text: String, color: Rgba) -> Div {
    let text: String = if text.chars().count() > 150 { format!("{}…", text.chars().take(150).collect::<String>().trim_end()) } else { text };
    div().text_size(px(11.0)).line_height(px(15.0)).text_color(color).child(text)
}

fn compact(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1} k", n as f64 / 1_000.0).replace('.', ","),
        _ => format!("{:.1} M", n as f64 / 1_000_000.0).replace('.', ","),
    }
}

fn small_button(id: (&'static str, usize), label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(24.0))
        .px_2()
        .flex()
        .items_center()
        .rounded(px(5.0))
        .border_1()
        .border_color(T.border_strong)
        .text_size(px(11.0))
        .text_color(T.text)
        .cursor_pointer()
        .hover(|d| d.bg(T.hover))
        .child(label)
}

/// Interrupteur marche / arrêt.
fn switch(id: (&'static str, usize), on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .w(px(28.0))
        .h(px(16.0))
        .p(px(2.0))
        .rounded(px(8.0))
        .flex()
        .when(on, |d| d.justify_end().bg(T.accent_bg))
        .when(!on, |d| d.justify_start().bg(T.raised).border_1().border_color(T.border_strong))
        .cursor_pointer()
        .child(div().size(px(12.0)).rounded_full().bg(if on { T.accent_on } else { T.text3 }))
}
