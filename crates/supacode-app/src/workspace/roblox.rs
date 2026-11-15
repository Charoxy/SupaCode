//! Onglet Roblox : projets Rojo (servis, ouverts dans Studio), outils installés, build de la
//! place, et skills Roblox (build, modèles 3D avec Blender, Luau) confiés aux agents.

use std::process::Child;

use supacode_core::extensions::{self, Scope};
use supacode_core::roblox::{self, RobloxProject, SkillStatus};

use super::*;

#[derive(Default)]
pub(super) struct RobloxState {
    pub projects: Vec<RobloxProject>,
    /// Projet servi par Rojo sur chaque port interrogé.
    pub serving: HashMap<u16, String>,
    /// Titres des fenêtres de Roblox Studio ouvertes.
    pub studio: Vec<String>,
    pub rojo: Option<PathBuf>,
    pub blender: Option<PathBuf>,
    pub skills: Vec<(&'static str, &'static str, SkillStatus)>,
    pub loaded: bool,
    pub loading: bool,
    /// `rojo serve` lancés par SupaCode, par dossier de projet.
    pub servers: HashMap<PathBuf, Child>,
    /// Projets en cours de build.
    pub building: HashSet<PathBuf>,
}

/// Demandes toutes prêtes confiées à un agent dans le projet Roblox choisi.
const AGENT_ACTIONS: [(&str, &str, &str); 5] = [
    (
        "roblox-construct",
        "Construire dans le jeu",
        "Utilise le skill roblox-build. Demande-moi ce qu'il faut construire dans la place ouverte dans Roblox Studio \
         (bâtiment, décor, zone, obby…) et dans quel style, puis construis-le avec les outils de Roblox Studio par passes \
         successives (maquette, formes et détails, matériaux, lumière), en regardant des captures d'écran à chaque passe \
         et en corrigeant avant de continuer.",
    ),
    (
        "roblox-model",
        "Modèle 3D avec Blender",
        "Utilise le skill roblox-blender-model. Demande-moi d'abord quel modèle créer (objet, style, taille approximative), \
         puis génère-le avec Blender en ligne de commande, montre-moi l'aperçu rendu et exporte-le dans le projet.",
    ),
    (
        "roblox-rojo",
        "Compiler et vérifier (Rojo)",
        "Utilise le skill roblox-rojo : vérifie la configuration Rojo du projet, installe les dépendances si besoin, \
         lance les vérifications disponibles (Selene, StyLua) et produis la place avec rojo build. \
         Résume ce qui passe, ce qui casse, et ce qui reste à tester dans Studio.",
    ),
    (
        "roblox-feature",
        "Nouvelle fonctionnalité Luau",
        "Utilise le skill roblox-luau. Demande-moi quelle fonctionnalité de jeu ajouter, puis implémente-la proprement \
         (serveur qui fait autorité, remotes validés, données sauvegardées si besoin) et dis-moi comment la tester dans Studio.",
    ),
    (
        "roblox-review",
        "Relire la sécurité des remotes",
        "Utilise le skill roblox-luau. Passe en revue tous les RemoteEvent et RemoteFunction du projet : \
         liste ceux qui font confiance au client (prix, dégâts, position, fréquence) et propose les corrections, sans les appliquer.",
    ),
];

impl Workspace {
    pub(super) fn refresh_roblox(&mut self, cx: &mut Context<Self>) {
        if self.roblox.loading {
            return;
        }
        self.roblox.loading = true;
        // Les `rojo serve` arrêtés d'eux-mêmes (erreur, port pris) sont oubliés.
        self.roblox.servers.retain(|_, child| matches!(child.try_wait(), Ok(None)));
        let roots: Vec<PathBuf> = self.projects.iter().map(|p| p.path.clone()).collect();
        let task = cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let projects: Vec<RobloxProject> = roots.iter().filter_map(|r| roblox::detect(r)).collect();
                    let mut ports: Vec<u16> = projects.iter().map(|p| p.serve_port).collect();
                    ports.push(roblox::DEFAULT_PORT);
                    ports.sort_unstable();
                    ports.dedup();
                    let serving = ports.into_iter().filter_map(|port| roblox::rojo_serving(port).map(|name| (port, name))).collect();
                    let skills_dir = extensions::skills_dir(None, Scope::User);
                    let skills = roblox::SKILLS
                        .iter()
                        .map(|s| (s.name, s.summary, skills_dir.as_deref().map_or(SkillStatus::Missing, |d| roblox::skill_status(d, s))))
                        .collect();
                    (projects, serving, roblox::studio_windows(), roblox::find_rojo(), roblox::find_blender(), skills)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let (projects, serving, studio, rojo, blender, skills) = found;
                let state = &mut this.roblox;
                (state.projects, state.serving, state.studio, state.rojo, state.blender, state.skills) = (projects, serving, studio, rojo, blender, skills);
                state.loaded = true;
                state.loading = false;
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    fn install_roblox_skills(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(dir) = extensions::skills_dir(None, Scope::User) else {
            self.toast("Dossier des skills introuvable.", cx);
            return false;
        };
        match roblox::install_skills(&dir) {
            Ok(count) => {
                self.toast(format!("{count} skills Roblox installés dans {}", dir.display()), cx);
                self.refresh_roblox(cx);
                self.refresh_extensions(cx);
                true
            }
            Err(error) => {
                self.toast(format!("Installation impossible : {error:#}"), cx);
                false
            }
        }
    }

    /// Projet Roblox visé par les actions : celui de la conversation ou du projet courant,
    /// sinon le premier trouvé.
    fn roblox_target(&self) -> Option<RobloxProject> {
        let current = self.current_project().map(|p| p.path);
        self.roblox.projects.iter().find(|p| current.as_ref().is_some_and(|c| same_path(c, &p.root))).or(self.roblox.projects.first()).cloned()
    }

    fn launch_roblox_agent(&mut self, prompt: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.roblox_target() else {
            self.toast("Aucun projet Roblox : ajoute d'abord le dossier de ton jeu.", cx);
            return;
        };
        if self.roblox.skills.iter().any(|(_, _, s)| *s != SkillStatus::Installed) && !self.install_roblox_skills(cx) {
            return;
        }
        let Some(project) = self.projects.iter().find(|p| same_path(&p.path, &target.root)).cloned() else { return };
        self.open_conversation(project, None, Some(prompt.to_owned()), window, cx);
    }

    fn toggle_rojo(&mut self, project: &RobloxProject, cx: &mut Context<Self>) {
        if let Some(mut child) = self.roblox.servers.remove(&project.root) {
            let _ = child.kill();
            let _ = child.wait();
            self.toast(format!("Rojo arrêté pour {}", project.name), cx);
        } else {
            match roblox::serve(project) {
                Ok(child) => {
                    self.roblox.servers.insert(project.root.clone(), child);
                    self.toast(format!("Rojo lancé pour {} (port {}) : connecte le plugin Rojo dans Studio.", project.name, project.serve_port), cx);
                }
                Err(error) => self.toast(format!("{error:#}"), cx),
            }
        }
        // Rojo met un instant à ouvrir son port.
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            let _ = this.update(cx, |this, cx| this.refresh_roblox(cx));
        });
        self._tasks.push(task);
        cx.notify();
    }

    fn build_and_open(&mut self, project: RobloxProject, cx: &mut Context<Self>) {
        if !self.roblox.building.insert(project.root.clone()) {
            return;
        }
        cx.notify();
        let task = cx.spawn(async move |this, cx| {
            let root = project.root.clone();
            let result = cx.background_executor().spawn(async move { roblox::build_place(&project).and_then(|place| roblox::open_file(&place).map(|_| place)) }).await;
            let _ = this.update(cx, |this, cx| {
                this.roblox.building.remove(&root);
                match result {
                    Ok(place) => this.toast(format!("Place construite et ouverte dans Studio : {}", place.display()), cx),
                    Err(error) => this.toast(format!("{error:#}"), cx),
                }
                this.refresh_roblox(cx);
            });
        });
        self._tasks.push(task);
    }

    pub(super) fn render_roblox(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = &self.roblox;
        let mut body = div().id("roblox").flex_1().min_h_0().overflow_y_scroll().px_3().pb_3().flex().flex_col().gap_4();

        // Outils
        let tool = |name: &'static str, path: &Option<PathBuf>, missing: &'static str| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().size(px(6.0)).rounded_full().bg(if path.is_some() { T.ok } else { T.warn }))
                .child(div().w(px(56.0)).flex_shrink_0().child(name))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(11.0))
                        .text_color(T.text3)
                        .child(path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| missing.to_owned())),
                )
        };
        let studio_text = match state.studio.len() {
            0 => "fermé".to_owned(),
            1 => state.studio[0].clone(),
            n => format!("{n} fenêtres ouvertes"),
        };
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .child(section_label("Outils"))
                .child(tool("Rojo", &state.rojo, "introuvable : rokit add rojo-rbx/rojo"))
                .child(tool("Blender", &state.blender, "introuvable (modèles 3D impossibles)"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().size(px(6.0)).rounded_full().bg(if state.studio.is_empty() { T.text3 } else { T.ok }))
                        .child(div().w(px(56.0)).flex_shrink_0().child("Studio"))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(11.0)).text_color(T.text3).child(studio_text)),
                ),
        );

        // Projets
        let mut projects = div().flex().flex_col().gap_1p5().child(section_label("Projets Roblox"));
        if !state.loaded {
            projects = projects.child(div().text_color(T.text3).child("Recherche…"));
        } else if state.projects.is_empty() {
            projects = projects.child(
                div()
                    .p_3()
                    .border_1()
                    .border_dashed()
                    .border_color(T.border_strong)
                    .rounded(px(6.0))
                    .text_size(px(12.0))
                    .text_color(T.text2)
                    .child("Aucun projet Roblox parmi tes projets. Ajoute le dossier d'un jeu qui contient un default.project.json (Rojo) ou une place .rbxl."),
            );
        }
        let current = self.current_project().map(|p| p.path);
        for (i, project) in state.projects.iter().enumerate() {
            let served = state.serving.get(&project.serve_port).is_some_and(|name| *name == project.name);
            let ours = state.servers.contains_key(&project.root);
            let in_studio = roblox::open_in_studio(project, &state.studio);
            let building = state.building.contains(&project.root);
            let selected = current.as_ref().is_some_and(|c| same_path(c, &project.root));
            let chip = |text: String, color: Rgba| div().px_1p5().rounded(px(4.0)).bg(T.app).text_size(px(11.0)).text_color(color).child(text);
            let action = |id: (&'static str, usize), label: String| {
                div()
                    .id(id)
                    .h(px(24.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded(px(5.0))
                    .border_1()
                    .border_color(T.border)
                    .text_size(px(11.0))
                    .text_color(T.text2)
                    .cursor_pointer()
                    .hover(|d| d.bg(T.hover).text_color(T.text))
                    .child(label)
            };
            let (for_rojo, for_build, root) = (project.clone(), project.clone(), project.root.clone());
            projects = projects.child(
                div()
                    .id(("roblox-project", i))
                    .p_2p5()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(if selected { T.claude } else { T.border })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_project = Some(root.display().to_string().to_lowercase());
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icon(icons::CUBE).size(px(13.0)).text_color(T.text2))
                            .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(project.name.clone())),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .child(if served { chip(format!("Rojo · port {}", project.serve_port), T.ok) } else { chip("Rojo arrêté".into(), T.text3) })
                            .child(if in_studio { chip("ouvert dans Studio".into(), T.ok) } else { chip("pas dans Studio".into(), T.text3) })
                            .when(project.project_file.is_none(), |d| d.child(chip("sans Rojo".into(), T.warn)))
                            .children(project.toolchain.iter().map(|t| chip((*t).to_owned(), T.text3))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .when(project.project_file.is_some(), |d| {
                                d.child(
                                    action(("roblox-serve", i), if ours { "Arrêter Rojo".into() } else if served { "Rojo déjà lancé".into() } else { "Lancer Rojo".into() })
                                        .when(ours || !served, |d| d.on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.toggle_rojo(&for_rojo, cx);
                                        }))),
                                )
                                .child(action(("roblox-build", i), if building { "Compilation…".into() } else { "Compiler + ouvrir dans Studio".into() }).on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.build_and_open(for_build.clone(), cx);
                                })))
                            })
                            .when_some(project.places.first().cloned().filter(|_| project.project_file.is_none()), |d, place| {
                                d.child(action(("roblox-open", i), "Ouvrir dans Studio".into()).on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    if let Err(error) = roblox::open_file(&place) {
                                        this.toast(format!("{error:#}"), cx);
                                    }
                                })))
                            }),
                    ),
            );
        }
        body = body.child(projects);

        // Agents et skills
        let target = self.roblox_target().map(|p| p.name).unwrap_or_else(|| "aucun projet".into());
        let mut agents = div().flex().flex_col().gap_1p5().child(section_label("Confier à un agent")).child(div().text_size(px(11.0)).text_color(T.text3).child(format!("Dans : {target}")));
        for (id, label, prompt) in AGENT_ACTIONS {
            agents = agents.child(
                div()
                    .id(id)
                    .h(px(30.0))
                    .px_2p5()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(6.0))
                    .bg(T.raised)
                    .cursor_pointer()
                    .hover(|d| d.bg(T.hover))
                    .on_click(cx.listener(move |this, _, window, cx| this.launch_roblox_agent(prompt, window, cx)))
                    .child(icon(icons::CLAUDE).size(px(12.0)).text_color(T.claude))
                    .child(div().flex_1().min_w_0().truncate().child(label)),
            );
        }
        body = body.child(agents);

        let all_installed = !state.skills.is_empty() && state.skills.iter().all(|(_, _, s)| *s == SkillStatus::Installed);
        let mut skills = div().flex().flex_col().gap_1p5().child(section_label("Skills Roblox"));
        for (name, summary, status) in &state.skills {
            let (text, color) = match status {
                SkillStatus::Installed => ("installé", T.ok),
                SkillStatus::Outdated => ("à mettre à jour", T.warn),
                SkillStatus::Missing => ("absent", T.text3),
            };
            skills = skills.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(div().flex().items_center().gap_2().child(div().flex_1().font_family(theme::MONO_FONT).text_size(px(12.0)).child(*name)).child(div().text_size(px(11.0)).text_color(color).child(text)))
                    .child(div().text_size(px(11.0)).text_color(T.text3).child(*summary)),
            );
        }
        skills = skills.child(
            div()
                .id("roblox-install-skills")
                .mt_1()
                .h(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.0))
                .border_1()
                .border_color(if all_installed { T.border } else { T.accent_bg })
                .text_size(px(12.0))
                .text_color(if all_installed { T.text3 } else { T.accent })
                .cursor_pointer()
                .hover(|d| d.bg(T.hover))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.install_roblox_skills(cx);
                }))
                .child(if all_installed { "Réinstaller les skills" } else { "Installer les skills Roblox" }),
        );
        body.child(skills)
    }
}
