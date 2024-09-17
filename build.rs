fn main() {
    println!("cargo:rustc-link-search=native=/home/lehtojo/Projects/kernel-loader/low/");
    println!("cargo:rustc-link-lib=static=boot"); 
}

