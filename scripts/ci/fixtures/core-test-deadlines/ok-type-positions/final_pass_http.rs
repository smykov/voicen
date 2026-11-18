use voicen_core::timeouts::Timeouts;

fn request(t: Timeouts, other: Option<Timeouts>) -> Vec<Timeouts> {
    let kept: Option<&Timeouts> = other.as_ref();
    let _ = kept;
    vec![t]
}

struct Holder { t: Timeouts }

fn none() {
    let t: Option<Timeouts> = None;
    let _ = t;
}
