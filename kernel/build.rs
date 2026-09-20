// build.rs — just signal rebuilds when assembly changes
fn main() {
    println!("cargo:rerun-if-changed=src/arch/boot.s");
    println!("cargo:rerun-if-changed=linker.ld");
}
