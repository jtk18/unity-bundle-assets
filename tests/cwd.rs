//! A bare file name has an empty parent folder, which must mean the current one. This binary
//! changes the working directory, so it runs alone, apart from the other tests.

mod common;
use common::*;
use unity_bundle_assets::Assets;

#[test]
fn a_bare_file_name_reads_streams_from_the_current_folder() {
    let dir = TempDir::new("cwd");
    let px = Pixels::Streamed {
        path: "t.assets.resS",
        offset: 0,
        size: 16,
    };
    let obj = texture(Layout::U2018_4, false, "t", 4, 4, format::ALPHA8, &px, &[]);
    dir.file(
        "t.assets",
        &serialized(17, "2018.4.36f1", false, 19, &[(7, TEXTURE_2D, obj)]),
    );
    dir.file("t.assets.resS", &[9; 16]);
    std::env::set_current_dir(&dir.0).unwrap();
    let a = Assets::open("t.assets").unwrap();
    assert_eq!(a.decode_texture(7).unwrap().rgba().len(), 64);
}
