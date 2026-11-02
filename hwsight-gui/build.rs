fn main() {
    slint_build::compile("ui/app.slint")
        .unwrap_or_else(|e| panic!("Slint UI failed to compile: {e}"));
}
