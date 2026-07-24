use std::env;
use std::path::Path;

fn main() {
    let command = env::current_exe()
        .ok()
        .as_deref()
        .and_then(Path::file_stem)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "fixture".to_owned());
    let arguments = env::args_os()
        .skip(1)
        .map(|value| value.to_string_lossy().replace('\\', "\\\\").replace('\n', "\\n"))
        .collect::<Vec<_>>();
    println!("fixture={command};argc={};argv={}", arguments.len(), arguments.join("|"));
}
