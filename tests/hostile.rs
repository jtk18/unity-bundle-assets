//! Files built to hurt: stream paths that leave the folder, offsets that overflow, objects that
//! alias, bombs, and callers handing back values from somewhere else. Each must give an error,
//! not a panic, a hang, or someone else's bytes.

mod common;
use common::*;
use unity_bundle_assets::{decode, Assets, Bundle, Error, LimitKind, Limits, SerializedFile};

fn streamed_from(
    dir: &std::path::Path,
    stream_path: &str,
    format_: i32,
    offset: u64,
    size: u32,
) -> Result<Vec<u8>, Error> {
    let px = Pixels::Streamed {
        path: stream_path,
        offset,
        size,
    };
    let obj = texture(Layout::U2018_4, false, "t", 4, 4, format_, &px, &[]);
    let path = dir.join("t.assets");
    std::fs::write(
        &path,
        serialized(17, "2018.4.36f1", false, 19, &[(7, TEXTURE_2D, obj)]),
    )
    .unwrap();
    Assets::open(&path)?
        .decode_texture(7)
        .map(unity_bundle_assets::Image::into_rgba)
}

fn alpha8(dir: &TempDir, stream_path: &str) -> Result<Vec<u8>, Error> {
    streamed_from(&dir.0, stream_path, format::ALPHA8, 0, 16)
}

#[test]
fn stream_paths_must_be_stream_file_names_beside_the_file() {
    let outer = TempDir::new("outer");
    outer.file("secret.resS", &[0xaa; 64]);
    let dir = TempDir::new("inner");
    dir.file(".netrc", &[0xaa; 64]);
    let absolute = outer.0.join("secret.resS");
    let relative = format!(
        "../{}/secret.resS",
        outer.0.file_name().unwrap().to_str().unwrap()
    );
    for path in [
        absolute.to_str().unwrap(),
        relative.as_str(),
        "sub/x.resS",
        ".netrc",
        "t.assets",
        ".",
    ] {
        match alpha8(&dir, path) {
            Err(Error::Unsupported(msg)) => assert!(msg.contains("not a .resS"), "{path}: {msg}"),
            other => panic!("{path}: {other:?}"),
        }
    }
    // Control: a .resS beside the asset is read.
    dir.file("ok.resS", &[0xaa; 16]);
    assert!(alpha8(&dir, "ok.resS").is_ok());
}

#[cfg(unix)]
#[test]
fn stream_files_may_not_be_symbolic_links() {
    let outer = TempDir::new("outer");
    outer.file("secret.txt", &[0xaa; 64]);
    let dir = TempDir::new("links");
    std::os::unix::fs::symlink(outer.0.join("secret.txt"), dir.0.join("x.resS")).unwrap();
    match alpha8(&dir, "x.resS") {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("regular"), "{msg}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn streams_must_be_regular_files_holding_the_bytes_claimed() {
    let dir = TempDir::new("irregular");
    std::fs::create_dir(dir.0.join("a_dir.resS")).unwrap();
    match alpha8(&dir, "a_dir.resS") {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("regular"), "{msg}"),
        other => panic!("{other:?}"),
    }
    dir.file("short.resS", &[1; 8]);
    assert!(matches!(alpha8(&dir, "short.resS"), Err(Error::Invalid(_))));
    assert!(matches!(
        streamed_from(&dir.0, "short.resS", format::ALPHA8, u64::MAX, 16),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        streamed_from(&dir.0, "short.resS", format::ALPHA8, 0, u32::MAX),
        Err(Error::Invalid(_))
    ));
    // A missing stream file names itself.
    match alpha8(&dir, "missing.resS") {
        Err(Error::Io { path: p, .. }) => assert!(p.ends_with("missing.resS")),
        other => panic!("{other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn sockets_are_refused_before_opening() {
    // Under /tmp: a socket path must fit in a few dozen bytes.
    let dir = std::path::PathBuf::from(format!("/tmp/uba-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let _sock = std::os::unix::net::UnixListener::bind(dir.join("s.resS")).unwrap();
    let result = streamed_from(&dir, "s.resS", format::ALPHA8, 0, 16);
    let _ = std::fs::remove_dir_all(&dir);
    match result {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("regular"), "{msg}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn unsupported_formats_are_refused_before_any_read() {
    // The stream file does not exist; a BC7 texture must fail on its format, not on I/O.
    let dir = TempDir::new("fmt");
    match streamed_from(&dir.0, "missing.resS", 25, 0, 16) {
        Err(Error::UnsupportedTextureFormat { format: 25, .. }) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn objects_may_not_overlap_or_share_an_id() {
    let tex = texture(
        Layout::U2018_4,
        false,
        "t",
        4,
        4,
        format::RGBA32,
        &Pixels::Inline(&rgba_4x4()),
        &[],
    );
    let file = serialized(
        17,
        "2018.4.36f1",
        false,
        19,
        &[(7, TEXTURE_2D, tex.clone()), (8, TEXTURE_2D, tex)],
    );
    assert!(SerializedFile::parse(file.clone()).is_ok());
    // Point the second object at the first one's bytes.
    let second = file
        .windows(8)
        .position(|w| w == 8i64.to_le_bytes())
        .unwrap()
        + 8;
    let mut aliased = file.clone();
    aliased[second..second + 4].copy_from_slice(&0u32.to_le_bytes());
    match SerializedFile::parse(aliased) {
        Err(Error::Invalid(msg)) => assert!(msg.contains("overlap"), "{msg}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
    // Give the second object the first one's ID.
    let mut twins = file;
    twins[second - 8..second].copy_from_slice(&7i64.to_le_bytes());
    match SerializedFile::parse(twins) {
        Err(Error::Invalid(msg)) => assert!(msg.contains("path ID 7"), "{msg}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
}

#[test]
fn object_offsets_that_overflow_are_errors() {
    let obj = texture(
        Layout::U2022_3,
        false,
        "t",
        4,
        4,
        format::RGBA32,
        &Pixels::Inline(&rgba_4x4()),
        &[],
    );
    let mut file = serialized(22, "2022.3.62f1", false, 19, &[(7, TEXTURE_2D, obj)]);
    let at = file
        .windows(8)
        .position(|w| w == 7i64.to_le_bytes())
        .unwrap()
        + 8;
    file[at..at + 8].copy_from_slice(&(u64::MAX - 3).to_le_bytes());
    assert!(matches!(
        SerializedFile::parse(file.clone()),
        Err(Error::Invalid(_))
    ));
    file[32..40].copy_from_slice(&u64::MAX.to_be_bytes());
    assert!(SerializedFile::parse(file).is_err());
}

#[test]
fn public_decode_refuses_impossible_sizes() {
    for f in [
        format::ALPHA8,
        format::RGB24,
        format::RGBA32,
        format::DXT1,
        format::DXT5,
    ] {
        assert!(decode::decode(f, u32::MAX, u32::MAX, &[]).is_err());
        assert!(decode::decode(f, 1 << 31, 1 << 31, &[0; 16]).is_err());
    }
    assert_eq!(decode::mip0_size(format::RGBA32, u32::MAX, u32::MAX), None);
}

#[test]
fn decompression_is_held_to_the_limit() {
    // 64 KiB of zeros in one LZMA block: small on disk, large once decompressed.
    let big = vec![0u8; 64 << 10];
    let mut opts = BundleOpts::new(6, "2018.4.36f1");
    opts.blocks = vec![1];
    let bomb = bundle(&opts, &[("data.resS", &big, 0)]);
    assert!(bomb.len() < big.len() / 4);
    let limits = Limits::DEFAULT.with_max_decompressed(32 << 10);
    assert!(matches!(
        Bundle::parse_with(&bomb, limits),
        Err(Error::LimitExceeded {
            kind: LimitKind::Decompressed,
            ..
        })
    ));
    assert!(Bundle::parse(&bomb).is_ok());
    // The directory's own claimed size counts too.
    let mut b = bomb;
    let at = b.windows(6).position(|w| w == b"36f1\0\0").unwrap() + 5 + 8 + 4;
    b[at..at + 4].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(matches!(
        Bundle::parse(&b),
        Err(Error::LimitExceeded { .. })
    ));
}

#[test]
fn the_directory_is_charged_at_its_parsed_size() {
    // 2,000 empty entries: 21 bytes each on disk, far more once parsed.
    let names: Vec<String> = (0..2000).map(|i| format!("e{i}")).collect();
    let entries: Vec<(&str, &[u8], u32)> = names.iter().map(|n| (n.as_str(), &[][..], 0)).collect();
    let b = bundle(&BundleOpts::new(6, "2018.4.36f1"), &entries);
    let on_disk = 2000 * 21;
    let limits = Limits::DEFAULT.with_max_decompressed(on_disk * 2);
    assert!(matches!(
        Bundle::parse_with(&b, limits),
        Err(Error::LimitExceeded {
            kind: LimitKind::Decompressed,
            ..
        })
    ));
    assert!(Bundle::parse(&b).is_ok());
}

#[test]
fn files_and_textures_over_the_limits_are_refused() {
    let dir = TempDir::new("limits");
    let obj = texture(
        Layout::U2018_4,
        false,
        "t",
        4,
        4,
        format::RGBA32,
        &Pixels::Inline(&rgba_4x4()),
        &[],
    );
    let path = dir.file(
        "t.assets",
        &serialized(17, "2018.4.36f1", false, 19, &[(7, TEXTURE_2D, obj)]),
    );
    let limits = Limits::DEFAULT.with_max_file_size(16);
    assert!(matches!(
        Assets::open_with(&path, limits),
        Err(Error::LimitExceeded {
            kind: LimitKind::FileSize,
            ..
        })
    ));
    let limits = Limits::DEFAULT.with_max_texture_pixels(15);
    let a = Assets::open_with(&path, limits).unwrap();
    assert!(matches!(
        a.decode_texture(7),
        Err(Error::LimitExceeded {
            kind: LimitKind::TexturePixels,
            ..
        })
    ));
    // A directory is not a file to open, and a missing file names itself.
    assert!(Assets::open(&dir.0).is_err());
    match Assets::open(dir.0.join("nope.assets")) {
        Err(Error::Io { path: p, .. }) => assert!(p.ends_with("nope.assets")),
        other => panic!("{other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn asset_files_the_caller_chose_may_be_symbolic_links() {
    let dir = TempDir::new("assetlink");
    let obj = texture(
        Layout::U2018_4,
        false,
        "t",
        4,
        4,
        format::RGBA32,
        &Pixels::Inline(&rgba_4x4()),
        &[],
    );
    let real = dir.file(
        "real.assets",
        &serialized(17, "2018.4.36f1", false, 19, &[(7, TEXTURE_2D, obj)]),
    );
    std::os::unix::fs::symlink(&real, dir.0.join("link.assets")).unwrap();
    assert!(Assets::open(&real).is_ok());
    assert!(Assets::open(dir.0.join("link.assets")).is_ok());
}

#[test]
fn values_from_another_file_do_not_panic() {
    let big = serialized(
        17,
        "2018.4.36f1",
        false,
        19,
        &[(
            7,
            TEXTURE_2D,
            texture(
                Layout::U2018_4,
                false,
                "t",
                64,
                64,
                format::RGBA32,
                &Pixels::Inline(&[0; 64 * 64 * 4]),
                &[],
            ),
        )],
    );
    let small = serialized(
        17,
        "2018.4.36f1",
        false,
        19,
        &[(
            7,
            TEXTURE_2D,
            texture(
                Layout::U2018_4,
                false,
                "t",
                4,
                4,
                format::RGBA32,
                &Pixels::Inline(&rgba_4x4()),
                &[],
            ),
        )],
    );
    let big = SerializedFile::parse(big).unwrap();
    let small = SerializedFile::parse(small).unwrap();
    let foreign = &big.objects()[0];
    assert!(small.bytes(foreign).is_none());
    assert!(small.name(foreign).is_none());

    let opts = BundleOpts::new(6, "2018.4.36f1");
    let large = Bundle::parse(&bundle(&opts, &[("a", &[0; 100], 0)])).unwrap();
    let tiny = Bundle::parse(&bundle(&opts, &[("a", &[0; 10], 0)])).unwrap();
    assert!(tiny.bytes(&large.entries()[0]).is_none());
}
