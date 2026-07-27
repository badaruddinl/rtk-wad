//! Versioned WSL-to-Windows dispatcher bridge payload.

use std::ffi::OsString;

use crate::config::OutputAdapterPreference;
use crate::paths::windows_path_to_wsl_path;

const WSL_BRIDGE_PREFIX: &str = "--wsl-bridge=";

pub(crate) struct WslBridgeRequest {
    pub(crate) distro: String,
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
        cwd,
        windows_cwd,
        extra_path,
        output_adapter,
        arguments @ ..,
    ] = fields.as_slice()
    else {
        return Err(
            "payload must contain protocol, distro, CWD, Windows CWD, extra path, adapter, and argv"
                .to_owned(),
        );
    };
    if protocol != "v2" {
        return Err("payload must use WSL bridge protocol v2".to_owned());
    }
    if distro.is_empty() || !cwd.starts_with('/') {
        return Err("payload must contain a WSL distro and an absolute Linux CWD".to_owned());
    }
    if !windows_cwd.is_empty() && windows_path_to_wsl_path(windows_cwd).is_none() {
        return Err("payload Windows CWD must be a drive-qualified path when set".to_owned());
    }
    let output_adapter = OutputAdapterPreference::parse(output_adapter)?;
    Ok(Some(WslBridgeRequest {
        distro: distro.clone(),
        cwd: cwd.clone(),
        windows_cwd: (!windows_cwd.is_empty()).then(|| windows_cwd.clone()),
        extra_path: (!extra_path.is_empty()).then(|| extra_path.clone()),
        output_adapter,
        arguments: arguments.iter().cloned().map(OsString::from).collect(),
    }))
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
