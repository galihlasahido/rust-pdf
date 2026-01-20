fn main() {
    // Tell cargo where to find the dynamic library
    // Adjust this path to where librust_pdf.dylib is located
    println!("cargo:rustc-link-search=native=../../target/release");
    println!("cargo:rustc-link-lib=dylib=rust_pdf");
}
