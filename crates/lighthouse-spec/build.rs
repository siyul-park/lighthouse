fn main() {
    for plugin in ["core", "design", "testing"] {
        println!("cargo:rerun-if-changed=../../plugins/{plugin}/decisions");
    }
}
