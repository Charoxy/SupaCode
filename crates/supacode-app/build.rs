//! Icône de l'exécutable sous Windows (explorateur, raccourcis, barre des tâches, fenêtre).

fn main() {
    println!("cargo:rerun-if-changed=assets/supacode.rc");
    println!("cargo:rerun-if-changed=assets/supacode.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/supacode.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("icône de SupaCode non compilée");
    }
}
