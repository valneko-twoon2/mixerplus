use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=app.ico");
    println!("cargo:rerun-if-changed=../LICENSE");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let source = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
        let manifest = source.join("app.manifest");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        // The linker accepts a compiled resource file directly, so the icon needs no
        // resource compiler or extra build dependency.
        let icon = std::fs::read(source.join("app.ico")).unwrap();
        let license = std::fs::read_to_string(source.join("../LICENSE")).unwrap();
        let copyright = license
            .lines()
            .find(|line| line.starts_with("Copyright"))
            .unwrap_or_default();
        let mut res = icon_resources(&icon);
        version_resource(&mut res, copyright);
        let resources = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("app.res");
        std::fs::write(&resources, res).unwrap();
        println!("cargo:rustc-link-arg={}", resources.display());
    }
}

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;

/// Converts an .ico file into a .res file holding one RT_ICON per image and the
/// RT_GROUP_ICON (ID 1) that Explorer and `LoadIcon` use.
fn icon_resources(ico: &[u8]) -> Vec<u8> {
    let word = |at: usize| u16::from_le_bytes([ico[at], ico[at + 1]]);
    let dword = |at: usize| u32::from_le_bytes(ico[at..at + 4].try_into().unwrap());
    let count = word(4) as usize;
    // A .res file starts with an empty entry that marks it as 32-bit.
    let mut out = Vec::new();
    resource(&mut out, 0, 0, 0, &[]);
    let mut group = vec![0, 0, 1, 0];
    group.extend_from_slice(&(count as u16).to_le_bytes());
    for index in 0..count {
        let entry = 6 + 16 * index;
        let size = dword(entry + 8) as usize;
        let offset = dword(entry + 12) as usize;
        let id = index as u16 + 1;
        resource(&mut out, RT_ICON, id, 0x1010, &ico[offset..offset + size]);
        // GRPICONDIRENTRY: the ICONDIRENTRY fields with the image offset replaced by an ID.
        group.extend_from_slice(&ico[entry..entry + 12]);
        group.extend_from_slice(&id.to_le_bytes());
    }
    resource(&mut out, RT_GROUP_ICON, 1, 0x1030, &group);
    out
}

/// Appends one resource with a numeric type and name, in the neutral language.
fn resource(out: &mut Vec<u8>, kind: u16, name: u16, flags: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    for value in [0xffff, kind, 0xffff, name] {
        out.extend_from_slice(&u16::to_le_bytes(value));
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(data);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn utf16z(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn pad(bytes: &mut Vec<u8>) {
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
}

/// One node of a version resource: header, key, value, then aligned children.
fn node(key: &str, value: &[u8], value_length: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![0; 6];
    out.extend(utf16z(key));
    pad(&mut out);
    out.extend_from_slice(value);
    for child in children {
        pad(&mut out);
        out.extend_from_slice(child);
    }
    let length = out.len() as u16;
    out[0..2].copy_from_slice(&length.to_le_bytes());
    out[2..4].copy_from_slice(&value_length.to_le_bytes());
    out[4..6].copy_from_slice(&u16::from(text).to_le_bytes());
    out
}

/// Appends a VERSIONINFO resource, so Explorer and Task Manager show the product
/// name (the same as the binary name, `mixerplus`) and version instead of the file name.
fn version_resource(out: &mut Vec<u8>, copyright: &str) {
    let part = |name: &str| -> u32 { std::env::var(name).unwrap().parse().unwrap() };
    let (major, minor, patch) = (
        part("CARGO_PKG_VERSION_MAJOR"),
        part("CARGO_PKG_VERSION_MINOR"),
        part("CARGO_PKG_VERSION_PATCH"),
    );
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut fixed = Vec::new();
    for value in [
        0xfeef04bd,
        0x0001_0000,
        major << 16 | minor,
        patch << 16,
        major << 16 | minor,
        patch << 16,
        0x3f,
        0,
        // VOS_NT_WINDOWS32 and VFT_APP.
        0x0004_0004,
        1,
        0,
        0,
        0,
    ] {
        fixed.extend_from_slice(&u32::to_le_bytes(value));
    }
    let strings: Vec<Vec<u8>> = [
        ("FileDescription", "mixerplus"),
        ("FileVersion", version.as_str()),
        ("InternalName", "mixerplus"),
        ("LegalCopyright", copyright),
        ("OriginalFilename", "mixerplus.exe"),
        ("ProductName", "mixerplus"),
        ("ProductVersion", version.as_str()),
    ]
    .iter()
    .map(|(key, value)| {
        let text = utf16z(value);
        node(key, &text, (text.len() / 2) as u16, true, &[])
    })
    .collect();
    // U.S. English with the Unicode code page.
    let table = node("040904b0", &[], 0, true, &strings);
    let string_info = node("StringFileInfo", &[], 0, true, &[table]);
    let translation = node("Translation", &0x04b0_0409u32.to_le_bytes(), 4, false, &[]);
    let var_info = node("VarFileInfo", &[], 0, true, &[translation]);
    let root = node(
        "VS_VERSION_INFO",
        &fixed,
        fixed.len() as u16,
        false,
        &[string_info, var_info],
    );
    resource(out, RT_VERSION, 1, 0x0030, &root);
}
