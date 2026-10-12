// Eseguito da cargo prima di compilare.
fn main() {
    // Traduce l'interfaccia .slint in codice Rust. I testi sono in italiano; la traduzione inglese
    // (translations/en/LC_MESSAGES/wheelor.po) viene incorporata nell'exe.
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".into())
        .with_bundled_translations("translations");
    slint_build::compile_with_config("ui/settings.slint", config).expect("compilazione dell'interfaccia Slint");

    // Risorse dentro l'exe:
    // - icona (risorsa 1), rigenerabile con tools/make-icon;
    // - manifest (DPI, permessi, versioni di Windows supportate);
    // - informazioni di versione, mostrate da Esplora file, Task Manager e dall'elenco delle app.
    //   FileVersion e ProductVersion arrivano da `version` in Cargo.toml.
    println!("cargo:rerun-if-changed=assets/wheelor.ico");
    println!("cargo:rerun-if-changed=assets/wheelor.manifest");
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("assets/wheelor.ico")
        .set_manifest_file("assets/wheelor.manifest")
        .set("FileDescription", "Wheelor")
        .set("ProductName", "Wheelor")
        .set("InternalName", "wheelor")
        .set("OriginalFilename", "wheelor.exe");
    resource.compile().expect("compilazione delle risorse Windows");
}
