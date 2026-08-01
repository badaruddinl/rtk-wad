use std::env;

pub(crate) fn trace(message: impl AsRef<str>) {
    if env::var("XUVA_WSL_TRACE").as_deref() == Ok("1") {
        eprintln!("xuva: trace: {}", message.as_ref());
    }
}
