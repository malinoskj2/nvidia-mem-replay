use super::*;

#[test]
fn corners_are_transparent_and_the_centre_is_the_glyph() {
    let size = 64;
    let pixels = rgba(size);
    assert_eq!(pixels.len(), (size * size * 4) as usize);

    let at = |x: u32, y: u32| {
        let index = ((y * size + x) * 4) as usize;
        [
            pixels[index],
            pixels[index + 1],
            pixels[index + 2],
            pixels[index + 3],
        ]
    };
    assert_eq!(at(0, 0)[3], 0);
    assert_eq!(at(size - 1, size - 1)[3], 0);
    assert_eq!(at(size / 2, size / 2), [0xFF, 0xFF, 0xFF, 0xFF]);
    assert_eq!(at(size / 8, size / 2), [0x2F, 0x9A, 0x5D, 0xFF]);
}

#[test]
fn ico_directory_lists_every_size_with_consistent_offsets() {
    let bytes = ico();
    assert_eq!(&bytes[..6], &[0, 0, 1, 0, 7, 0]);

    let mut expected_offset = 6 + 16 * ICO_SIZES.len();
    for (index, size) in ICO_SIZES.iter().enumerate() {
        let entry = &bytes[6 + 16 * index..6 + 16 * (index + 1)];
        assert_eq!(u32::from(entry[0]), size % 256);
        let length = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
        let offset = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
        assert_eq!(offset, expected_offset);
        assert_eq!(&bytes[offset..offset + 4], &40_u32.to_le_bytes());
        expected_offset += length;
    }
    assert_eq!(expected_offset, bytes.len());
}

#[test]
fn committed_ico_matches_the_renderer() {
    let committed: &[u8] = include_bytes!("../../../assets/nvidia-mem-replay.ico");
    assert!(
        committed == ico().as_slice(),
        "regenerate assets/nvidia-mem-replay.ico with `nvidia-mem-replay icon assets/nvidia-mem-replay.ico`"
    );
}
