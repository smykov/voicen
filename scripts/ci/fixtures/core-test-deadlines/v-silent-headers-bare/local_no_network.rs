use Serve::SilentHeaders;

fn t() {
    let plan = SilentHeaders { hold: Duration::from_secs(3) };
}
