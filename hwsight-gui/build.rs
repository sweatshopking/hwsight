fn main() {
    // Translations are bundled into the binary from ../po (shared with hwsight-core, see
    // po/README.md). No translation context, so the same text is translated once.
    let config = slint_build::CompilerConfiguration::new()
        .with_bundled_translations("../po")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/app.slint", config)
        .unwrap_or_else(|e| panic!("Slint UI failed to compile: {e}"));
}
