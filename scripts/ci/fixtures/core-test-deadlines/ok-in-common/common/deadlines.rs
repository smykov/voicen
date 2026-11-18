// The deadline constructors live here only.
use std::time::Duration;
use voicen_core::timeouts::Timeouts;

pub fn in_time() -> Timeouts {
    Timeouts {
        connect: Duration::from_secs(10),
        ..Timeouts::default()
    }
}

pub fn trickle() -> Serve {
    Serve::Trickle { gap: Duration::from_secs(1) }
}
pub fn stall() -> Serve {
    Serve::Stall { hold: Duration::from_secs(30) }
}
pub fn silent() -> Serve {
    Serve::SilentHeaders { hold: Duration::from_secs(30) }
}
