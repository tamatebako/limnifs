//! Integration cover for the `defaults.shared_inline` knob
//! (limnifs#189): with the knob off the writer emits no shared-inline
//! table and never sets `INODE_FLAG_SHARED_INLINE` (0x08), so the image
//! mounts on pre-0.2.53 readers whose reserved-flag mask rejects that
//! bit (limnifs#186). Both ways must round-trip content byte-exact
//! through the limnifs-core reader path.

use limnifs_core::{
    parse_feature_flags_section, parse_manifest_header, parse_metadata_blob,
    parse_metadata_reference, ContentHandle, ManifestCursor,
};
use limnifs_write::{write_directory_with_config, WriteConfig};

const PAYLOAD: [u8; 1024] = [0xA5; 1024];

fn fixture_tree() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("limnifs-inline-gate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    for name in ["dup-a.bin", "dup-b.bin", "dup-c.bin", "dup-d.bin"] {
        std::fs::write(dir.join(name), PAYLOAD).expect("write");
    }
    dir
}

/// Parse the manifest exactly as a consumer would; returns the inline
/// metadata blob length plus every file's content.
fn read_back(image: &[u8]) -> (usize, Vec<Vec<u8>>) {
    let mut c = ManifestCursor::new(image);
    parse_manifest_header(&mut c).expect("header");
    parse_feature_flags_section(&mut c).expect("flags");
    let mr = parse_metadata_reference(&mut c).expect("metadata reference");
    let bytes = mr.inline_metadata.as_deref().expect("metadata inlined");
    let blob = parse_metadata_blob(&mut ManifestCursor::new(bytes)).expect("blob parses");
    let mut contents: Vec<Vec<u8>> = blob
        .inodes
        .iter()
        .filter_map(|i| match &i.content_handle {
            ContentHandle::InlineData(d) => Some(d.clone()),
            // Post-parse every shared ref is resolved; one surviving
            // here means the image references a table it does not carry.
            ContentHandle::SharedInline(idx) => {
                panic!("unresolved shared-inline reference {idx}")
            }
            _ => None, // directories, symlinks, slice-mapped files
        })
        .collect();
    contents.sort();
    (bytes.len(), contents)
}

#[test]
fn shared_inline_gate_off_emits_no_table_and_round_trips() {
    let dir = fixture_tree();

    let on = write_directory_with_config(&dir, &WriteConfig::default_v0_1()).expect("write on");
    let mut cfg = WriteConfig::default_v0_1();
    cfg.defaults.shared_inline = false;
    let off = write_directory_with_config(&dir, &cfg).expect("write off");
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(on.drop_count, 0, "fixture stays fully inline");
    assert_eq!(off.drop_count, 0, "fixture stays fully inline");

    let (on_len, on_contents) = read_back(&on.bytes);
    let (off_len, off_contents) = read_back(&off.bytes);

    // Both ways serve every byte.
    assert_eq!(on_contents, vec![PAYLOAD.to_vec(); 4]);
    assert_eq!(off_contents, vec![PAYLOAD.to_vec(); 4]);

    // The knob-off blob re-inlines every duplicate instead of
    // referencing the (absent) table: four inline bodies vs one table
    // entry + 5-byte refs, so it must be larger by ≥ two payload
    // copies (actual gap ≈ 3 KB at this fixture size).
    assert!(
        off_len >= on_len + 2 * PAYLOAD.len(),
        "knob-off blob ({off_len}) must re-inline the duplicates (knob-on blob {on_len})"
    );
}
