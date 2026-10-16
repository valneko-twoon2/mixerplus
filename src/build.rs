use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=app.ico");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let source = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
        let manifest = source.join("app.manifest");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        // The linker accepts a compiled resource file directly, so the icon needs no
        // resource compiler or extra build dependency.
        let icon = std::fs::read(source.join("app.ico")).unwrap();
        let resources = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("app.res");
        std::fs::write(&resources, icon_resources(&icon)).unwrap();
        println!("cargo:rustc-link-arg={}", resources.display());
    }
}

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;

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
