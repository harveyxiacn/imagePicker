//! DC-only decoder vs. the reference scaled decode.

#[test]
fn dc_only_decoder_matches_reference_scaled_decode() {
    use jpeg_encoder::{ColorType, Encoder, SamplingFactor};
    let (w, h) = (803u32, 517u32); // not multiples of 8/16 on purpose
    let rgb: Vec<u8> = (0..h)
        .flat_map(|y| {
            (0..w).flat_map(move |x| {
                [
                    (x * 255 / w) as u8,
                    (y * 255 / h) as u8,
                    ((x + y) * 255 / (w + h)) as u8,
                ]
            })
        })
        .collect();
    let gray: Vec<u8> = rgb.chunks(3).map(|p| p[0]).collect();
    let cases = [
        (SamplingFactor::F_1_1, 0u16, false),
        (SamplingFactor::F_2_2, 0, false),
        (SamplingFactor::F_2_1, 7, false),
        (SamplingFactor::F_1_2, 0, false),
        (SamplingFactor::F_2_2, 3, false),
        (SamplingFactor::F_1_1, 0, true),
    ];
    for (sf, ri, is_gray) in cases {
        let mut out = Vec::new();
        let mut enc = Encoder::new(&mut out, 85);
        enc.set_sampling_factor(sf);
        if ri > 0 {
            enc.set_restart_interval(ri);
        }
        if is_gray {
            enc.encode(&gray, w as u16, h as u16, ColorType::Luma)
                .unwrap();
        } else {
            enc.encode(&rgb, w as u16, h as u16, ColorType::Rgb)
                .unwrap();
        }
        let (dw, dh, mine) = super::dcjpeg::decode_eighth(&out).expect("dc decode");
        assert_eq!((dw, dh), (101, 65));
        let mut d = jpeg_decoder::Decoder::new(&out[..]);
        d.read_info().unwrap();
        assert_eq!(d.scale(100, 64).unwrap(), (101, 65));
        let px = d.decode().unwrap();
        let reference: Vec<u8> = if is_gray {
            px.iter().flat_map(|&g| [g, g, g]).collect()
        } else {
            px
        };
        assert_eq!(reference.len(), mine.len());
        let mad = mine
            .iter()
            .zip(&reference)
            .map(|(a, b)| (*a as i32 - *b as i32).abs() as f64)
            .sum::<f64>()
            / mine.len() as f64;
        assert!(
            mad < 4.0,
            "mean abs diff {mad} for sampling {sf:?} ri={ri} gray={is_gray}"
        );
    }
    // progressive / garbage -> None (fallback path)
    assert!(super::dcjpeg::decode_eighth(b"\xFF\xD8\xFF\xC2\x00\x02").is_none());
    assert!(super::dcjpeg::decode_eighth(&[0u8; 10]).is_none());
}
