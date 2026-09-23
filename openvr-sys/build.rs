use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let headers = manifest_dir.join("openvr").join("headers");

    // The bindings are generated from the vendored SDK headers, which can be newer
    // than the OpenVR client library installed on the system: the library only
    // provides the loader entry points, while the interfaces are negotiated with
    // the runtime through the version strings defined in the headers.
    autocxx_build::Builder::new("src/lib.rs", [&headers])
        .extra_clang_args(&["-std=c++14"])
        .build()
        .unwrap()
        .flag_if_supported("-std=c++14")
        .compile("openvr_glue");

    // Link the OpenVR client library provided by the system. Only the link flags
    // are used, the include path would point to the older system headers.
    pkg_config::probe_library("openvr").unwrap();

    write_event_type_conversion(&headers.join("openvr.h"), &out_dir.join("event_type.rs"));

    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=openvr/headers/openvr.h");
}

/// Generate `TryFrom<u32> for EVREventType` from the header, so that the event
/// types always match the SDK the bindings are generated from.
fn write_event_type_conversion(header: &Path, out: &Path) {
    let header = std::fs::read_to_string(header).unwrap();
    let start = header
        .find("enum EVREventType")
        .expect("EVREventType not found");
    let body = &header[start..];
    let body = &body[body.find('{').unwrap() + 1..body.find("};").unwrap()];

    let mut arms = String::new();
    for line in body.lines() {
        let line = line.split("//").next().unwrap().trim();
        let Some((name, value)) = line.trim_end_matches(',').split_once('=') else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        let value: u32 = value
            .parse()
            .unwrap_or_else(|_| panic!("unexpected value for {name}: {value}"));
        writeln!(arms, "            {value} => EVREventType::{name},").unwrap();
    }

    let code = format!(
        "impl TryFrom<u32> for EVREventType {{
    type Error = ();
    fn try_from(value: u32) -> Result<Self, Self::Error> {{
        Ok(match value {{
{arms}            _ => return Err(()),
        }})
    }}
}}
"
    );
    std::fs::write(out, code).unwrap();
}
