fn plans() {
    let a = Serve::Full;
    let b = Serve::Status(503);
    let c = Serve::Redirect("/other".to_string());
    let _ = (a, b, c);
}
