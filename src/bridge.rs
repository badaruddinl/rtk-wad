//! Versioned WSL-to-Windows dispatcher bridge payload.

use std::ffi::OsString;

use crate::config::{OutputAdapterPreference, validate_linux_path_list, validate_wsl_user};
use crate::paths::windows_path_to_wsl_path;

const WSL_BRIDGE_PREFIX: &str = "--wsl-bridge=";

pub(crate) struct WslBridgeRequest {
    pub(crate) distro: String,
    pub(crate) origin_user: String,
    pub(crate) cwd: String,
    pub(crate) windows_cwd: Option<String>,
    pub(crate) extra_path: Option<String>,
    pub(crate) output_adapter: OutputAdapterPreference,
    pub(crate) arguments: Vec<OsString>,
}

pub(crate) fn wsl_bridge_request(
    arguments: &[OsString],
) -> Result<Option<WslBridgeRequest>, String> {
    if arguments.len() != 1 {
        return Ok(None);
    }
    let Some(encoded) = arguments[0]
        .to_str()
        .and_then(|argument| argument.strip_prefix(WSL_BRIDGE_PREFIX))
    else {
        return Ok(None);
    };
    let fields = decode_wsl_bridge_fields(encoded)?;
    let [
        protocol,
        distro,
        origin_user,
        cwd,
        windows_cwd,
        extra_path,
        output_adapter,
        arguments @ ..,
    ] = fields.as_slice()
    else {
        return Err(
            "payload must contain protocol, distro, origin user, CWD, Windows CWD, extra path, adapter, and argv"
                .to_owned(),
        );
    };
    if protocol != "v3" {
        return Err("payload must use WSL bridge protocol v3".to_owned());
    }
    if distro.is_empty() || !cwd.starts_with('/') {
        return Err("payload must contain a WSL distro and an absolute Linux CWD".to_owned());
    }
    if !windows_cwd.is_empty() && !bridge_windows_cwd_is_valid(distro, cwd, windows_cwd) {
        return Err(
            "payload Windows CWD must be a drive path or a matching WSL UNC mapping".to_owned(),
        );
    }
    validate_wsl_user(origin_user)?;
    if !extra_path.is_empty() {
        validate_linux_path_list(extra_path, "bridge extra path")?;
    }
    let output_adapter = OutputAdapterPreference::parse(output_adapter)?;
    Ok(Some(WslBridgeRequest {
        distro: distro.clone(),
        origin_user: origin_user.clone(),
        cwd: cwd.clone(),
        windows_cwd: (!windows_cwd.is_empty()).then(|| windows_cwd.clone()),
        extra_path: (!extra_path.is_empty()).then(|| extra_path.clone()),
        output_adapter,
        arguments: arguments.iter().cloned().map(OsString::from).collect(),
    }))
}

fn bridge_windows_cwd_is_valid(distro: &str, cwd: &str, windows_cwd: &str) -> bool {
    if windows_path_to_wsl_path(windows_cwd).is_some() {
        // `wslpath -w` is authoritative for the originating distro. Some
        // distros expose Windows drives under a nonstandard Linux mount root,
        // so reconstructing `/mnt/<drive>` here would reject a real mapping.
        return true;
    }

    let normalized = windows_cwd.replace('/', "\\");
    ["\\\\wsl.localhost\\", "\\\\wsl$\\"]
        .into_iter()
        .find_map(|prefix| normalized.strip_prefix(prefix))
        .and_then(|remainder| remainder.split_once('\\'))
        .is_some_and(|(mapped_distro, mapped_path)| {
            mapped_distro.eq_ignore_ascii_case(distro)
                && format!("/{}", mapped_path.replace('\\', "/")) == cwd
        })
}

pub(crate) fn decode_wsl_bridge_fields(encoded: &str) -> Result<Vec<String>, String> {
    let bytes = decode_base64(encoded)?;
    if bytes.last() != Some(&0) {
        return Err("payload must end with a NUL terminator".to_owned());
    }
    bytes[..bytes.len() - 1]
        .split(|byte| *byte == 0)
        .map(|argument| {
            let argument = std::str::from_utf8(argument)
                .map_err(|_| "payload contains non-UTF-8 argv".to_owned())?;
            Ok(argument.to_owned())
        })
        .collect()
}

fn decode_base64(encoded: &str) -> Result<Vec<u8>, String> {
    let compact = encoded
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    if compact.is_empty() || compact.len() % 4 != 0 {
        return Err("payload is not padded base64".to_owned());
    }
    let mut decoded = Vec::with_capacity(compact.len() / 4 * 3);
    for (index, quartet) in compact.chunks_exact(4).enumerate() {
        let final_quartet = index == compact.len() / 4 - 1;
        let padding = quartet
            .iter()
            .rev()
            .take_while(|byte| **byte == b'=')
            .count();
        if padding > 2 || (!final_quartet && padding != 0) {
            return Err("payload has invalid base64 padding".to_owned());
        }
        let values = quartet
            .iter()
            .enumerate()
            .map(|(position, byte)| match byte {
                b'A'..=b'Z' => Ok(byte - b'A'),
                b'a'..=b'z' => Ok(byte - b'a' + 26),
                b'0'..=b'9' => Ok(byte - b'0' + 52),
                b'+' => Ok(62),
                b'/' => Ok(63),
                b'=' if position >= 2 => Ok(0),
                _ => Err("payload contains non-base64 data".to_owned()),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if quartet[2] == b'=' && quartet[3] != b'=' {
            return Err("payload has invalid base64 padding".to_owned());
        }
        decoded.push((values[0] << 2) | (values[1] >> 4));
        if padding < 2 {
            decoded.push((values[1] << 4) | (values[2] >> 2));
        }
        if padding == 0 {
            decoded.push((values[2] << 6) | values[3]);
        }
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::{bridge_windows_cwd_is_valid, decode_wsl_bridge_fields, wsl_bridge_request};
    use std::ffi::OsString;

    fn encode_fields(fields: &[&str]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut bytes = Vec::new();
        for field in fields {
            bytes.extend_from_slice(field.as_bytes());
            bytes.push(0);
        }
        let mut encoded = String::new();
        for chunk in bytes.chunks(3) {
            let first = chunk[0];
            let second = chunk.get(1).copied().unwrap_or(0);
            let third = chunk.get(2).copied().unwrap_or(0);
            encoded.push(ALPHABET[(first >> 2) as usize] as char);
            encoded.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
            encoded.push(if chunk.len() > 1 {
                ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char
            } else {
                '='
            });
            encoded.push(if chunk.len() > 2 {
                ALPHABET[(third & 0x3f) as usize] as char
            } else {
                '='
            });
        }
        encoded
    }

    #[test]
    fn bridge_cwd_accepts_only_exact_drive_or_matching_unc_mappings() {
        assert!(bridge_windows_cwd_is_valid(
            "Ubuntu",
            "/custom/windows-drive/work",
            r"E:\work"
        ));
        assert!(bridge_windows_cwd_is_valid(
            "Ubuntu",
            "/home/user/work",
            r"\\wsl.localhost\Ubuntu\home\user\work"
        ));
        assert!(!bridge_windows_cwd_is_valid(
            "Ubuntu",
            "/home/user/work",
            r"\\wsl.localhost\Other\home\user\work"
        ));
        assert!(!bridge_windows_cwd_is_valid(
            "Ubuntu",
            "/home/user/work",
            r"\\wsl.localhost\Ubuntu\home\user\other"
        ));
    }

    #[test]
    fn bridge_rejects_unvalidated_extra_path_from_the_originating_shell() {
        let error = match wsl_bridge_request(&[OsString::from(
            "--wsl-bridge=djMAVWJ1bnR1AGJhZGFyAC9tbnQvZC9maXh0dXJlAEQ6XGZpeHR1cmUAcmVsYXRpdmU6L29wdAByYXcAbm9kZQA=",
        )]) {
            Ok(_) => panic!("relative bridge extra path must be rejected"),
            Err(error) => error,
        };
        assert!(error.contains("normalized absolute Linux paths"));
    }

    #[test]
    fn generated_bridge_payloads_round_trip_literal_utf8_fields() {
        let literals = [
            "",
            "plain",
            "with spaces",
            "quote\"single'",
            "dollar$pipe|amp&semi;",
            "back\\slash/forward",
            "unicode-λ-雪",
        ];
        for literal in literals {
            let fields = [
                "v3",
                "Ubuntu",
                "runner",
                "/home/runner/work",
                r"\\wsl.localhost\Ubuntu\home\runner\work",
                "",
                "auto",
                "rg",
                literal,
            ];
            let encoded = encode_fields(&fields);
            assert_eq!(
                decode_wsl_bridge_fields(&encoded).expect("generated payload decodes"),
                fields.map(str::to_owned)
            );
        }
    }

    #[test]
    fn corrupted_bridge_payload_bytes_fail_closed_without_panicking() {
        let encoded = encode_fields(&["v3", "Ubuntu", "runner", "/work", "", "", "auto", "rg"]);
        for index in 0..encoded.len() {
            if encoded.as_bytes()[index] == b'=' {
                continue;
            }
            let mut corrupted = encoded.clone().into_bytes();
            corrupted[index] = b'!';
            let corrupted = String::from_utf8(corrupted).expect("ASCII corruption remains UTF-8");
            assert!(
                decode_wsl_bridge_fields(&corrupted).is_err(),
                "index={index}"
            );
        }
        for truncated in 1..=3 {
            assert!(decode_wsl_bridge_fields(&encoded[..encoded.len() - truncated]).is_err());
        }
    }
}
