fn main() {
    println!("cargo:rerun-if-changed=../../bundles");
    println!("cargo:rerun-if-changed=../../recur-skills");
}
