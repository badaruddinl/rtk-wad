use std::ffi::OsString;

pub(crate) fn exec_prefix(distro: &str, user: Option<&str>) -> Vec<OsString> {
    let mut arguments = vec![OsString::from("-d"), OsString::from(distro)];
    if let Some(user) = user {
        arguments.extend([OsString::from("-u"), OsString::from(user)]);
    }
    arguments.push(OsString::from("--exec"));
    arguments
}

pub(crate) fn valid_installation_id(installation_id: &str) -> bool {
    installation_id.len() == 36
        && installation_id.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23) && byte == b'-'
                || !matches!(index, 8 | 13 | 18 | 23) && byte.is_ascii_hexdigit()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installation_identity_is_exact_and_bounded() {
        assert!(valid_installation_id(
            "01234567-89ab-cdef-0123-456789abcdef"
        ));
        for invalid in [
            "",
            "01234567-89ab-cdef-0123-456789abcdeg",
            "0123456789ab-cdef-0123-456789abcdef",
        ] {
            assert!(!valid_installation_id(invalid));
        }
    }
}
pub(crate) mod arguments;
pub(crate) mod authorization;
pub(crate) mod cancellation;
#[cfg(test)]
#[path = "tests.rs"]
mod integration_tests;
pub(crate) mod lifecycle;
pub(crate) mod supervisor;
pub(crate) mod test_hooks;
