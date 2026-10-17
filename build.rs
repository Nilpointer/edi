// Embeds the app icon into edi.exe when building on Windows.
fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/edi.ico");
        res.compile().expect("compile Windows resources");
    }
}
