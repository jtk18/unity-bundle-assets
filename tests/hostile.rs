//! Files built to hurt: stream paths that leave the folder, offsets that overflow, bombs, and
//! callers handing back values from somewhere else. Each must give an error, not a panic, a
//! hang, or someone else's bytes.

mod common;
use common::*;
use unity_bundle_assets::{decode, Assets, Bundle, Error, Limits, SerializedFile};

fn streamed_from(
    dir: &TempDir,
    stream_path: &str,
    offset: u64,
    size: u32,
) -> Result<Vec<u8>, Error> {
    let px = Pixels::Streamed {
        path: stream_path,
        offset,
        size,
    };
    let obj = texture(Layout::U2018_4, false, "t", 4, 4, format::ALPHA8, &px, &[]);
    let path = dir.file(
        "t.assets",
        &serialized(17, "2018.4.36f1", false, 19, &[(7, TEXTURE_2D, obj)]),
    );
    Assets::open(&path)?.decode_texture(7).map(|i| i.rgba)
}

#[test]
fn stream_paths_cannot_leave_the_folder() {
    let outer = TempDir::new("outer");
    outer.file("secret.bin", &[0xaa; 64]);
    let dir = TempDir::new("inner");
    let secret = outer.0.join("secret.bin");
    let relative = format!(
        "../{}/secret.bin",
        outer.0.file_name().unwrap().to_str().unwrap()
    );
    for path in [
        secret.to_str().unwrap(),
        relative.as_str(),
        "sub/secret.bin",
        "/etc/hosts",
        ".",
        "..",
    ] {
        match streamed_from(&dir, path, 0, 16) {
            Err(Error::Unsupported(msg)) => {
                assert!(msg.contains("not a file name"), "{path}: {msg}")
            }
            other => panic!("{path}: {other:?}"),
        }
    }
    // Control: the same file beside the asset is read.
    dir.file("ok.resS", &[0xaa; 16]);
    assert!(streamed_from(&dir, "ok.resS", 0, 16).is_ok());
}

#[test]
fn streams_must_be_regular_files_holding_the_bytes_claimed() {
    let dir = TempDir::new("irregular");
    std::fs::create_dir(dir.0.join("a_dir.resS")).unwrap();
    match streamed_from(&dir, "a_dir.resS", 0, 16) {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("regular"), "{msg}"),
        other => panic!("{other:?}"),
    }
    // A socket: opening it would fail differently; it must be refused before that.
    #[cfg(unix)]
    {
        let _sock = std::os::unix::net::UnixListener::bind(dir.0.join("sock.resS")).unwrap();
        match streamed_from(&dir, "sock.resS", 0, 16) {
            Err(Error::Unsupported(msg)) => assert!(msg.contains("regular"), "{msg}"),
            other => panic!("{other:?}"),
        }
    }
    dir.file("short.resS", &[1; 8]);
    assert!(matches!(
        streamed_from(&dir, "short.resS", 0, 16),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        streamed_from(&dir, "short.resS", u64::MAX, 16),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        streamed_from(&dir, "short.resS", 0, u32::MAX),
        Err(Error::Invalid(_))
    ));
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
    // The object's start is the u64 after its path ID (7); set it near the top.
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
    // And the data offset in the header.
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
    let mut limits = Limits::default();
    limits.max_decompressed = 32 << 10;
    assert!(matches!(
        Bundle::parse_with(&bomb, limits),
        Err(Error::LimitExceeded {
            what: "bundle decompressed size",
            ..
        })
    ));
    assert!(Bundle::parse(&bomb).is_ok());
    // The directory's own claimed size counts too.
    let mut b = bomb.clone();
    let at = b.windows(6).position(|w| w == b"36f1\0\0").unwrap() + 5 + 8 + 4;
    b[at..at + 4].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(matches!(
        Bundle::parse(&b),
        Err(Error::LimitExceeded { .. })
    ));
}

#[test]
fn lz4_blocks_cannot_claim_impossible_sizes() {
    let mut opts = BundleOpts::new(6, "2018.4.36f1");
    opts.blocks = vec![2];
    let mut b = bundle(&opts, &[("data.resS", &[1, 2, 3], 0)]);
    // Find the block table's uncompressed size (3) followed by the compressed size (4).
    let at = b
        .windows(8)
        .position(|w| w == [0, 0, 0, 3, 0, 0, 0, 4])
        .unwrap();
    b[at..at + 4].copy_from_slice(&(512u32 << 20).to_be_bytes());
    let err = Bundle::parse(&b).unwrap_err();
    assert!(
        matches!(err, Error::Invalid(_) | Error::LimitExceeded { .. }),
        "{err:?}"
    );
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
    let mut limits = Limits::default();
    limits.max_file_size = 16;
    assert!(matches!(
        Assets::open_with(&path, limits),
        Err(Error::LimitExceeded {
            what: "file size",
            ..
        })
    ));
    let mut limits = Limits::default();
    limits.max_texture_pixels = 15;
    let a = Assets::open_with(&path, limits).unwrap();
    assert!(matches!(
        a.decode_texture(7),
        Err(Error::LimitExceeded {
            what: "texture pixels",
            ..
        })
    ));
    // A directory is not a file to open.
    assert!(Assets::open(&dir.0).is_err());
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
