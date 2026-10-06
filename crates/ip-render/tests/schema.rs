use ip_render::{EditStack, MaskRef, MaskTarget, Op};

#[test]
fn parses_doc05_stack_and_ignores_future_ops() {
    let json = r#"{"version":1,"ops":[
        {"type":"crop","rect":[0.02,0.05,0.96,0.9],"angle":-1.3,"aspect":"4:5"},
        {"type":"patch","kind":"best_take","person_id":12,"asset":"x.webp"},
        {"type":"warp","kind":"face_slim","strength":0.25},
        {"type":"global","exposure":0.35,"contrast":12,"temp":300,
         "curve":{"rgb":[[0,0],[0.25,0.22],[0.75,0.8],[1,1]]},
         "hsl":{"orange":{"h":0,"s":-5,"l":8}},
         "grading":{"shadows":[220,0.1],"highlights":[40,0.08]},"source":"ai_auto@1"},
        {"type":"local","mask":{"kind":"ai","target":"sky"},"adjust":{"exposure":-0.3,"saturation":10}},
        {"type":"lut","file":"film_warm","amount":0.6},
        {"type":"output_sharpen","amount":20}
    ]}"#;
    let s: EditStack = serde_json::from_str(json).unwrap();
    assert_eq!(s.ops.len(), 7);
    assert!(matches!(s.ops[1], Op::Unknown) && matches!(s.ops[2], Op::Unknown));
    match &s.ops[4] {
        Op::Local(l) => assert_eq!(
            l.mask,
            MaskRef::Ai {
                target: MaskTarget::Sky,
                person_id: None
            }
        ),
        o => panic!("{o:?}"),
    }
    assert!(!s.is_identity());
    assert!(serde_json::from_str::<EditStack>("{}")
        .unwrap()
        .is_identity());
}

#[test]
fn auto_adjust_values_are_slider_granular() {
    // A dark, bluish image produces non-trivial suggestions.
    let (w, h) = (64u32, 48u32);
    let data: Vec<u8> = (0..w * h)
        .flat_map(|i| [20u8, 30, (60 + i % 40) as u8])
        .collect();
    let img = ip_render::RgbImage {
        width: w,
        height: h,
        data,
    };
    let a = ip_render::auto_adjust(&img, &Default::default(), ip_render::AutoMode::Auto);
    assert_eq!((a.exposure * 100.0).round() / 100.0, a.exposure);
    assert_eq!((a.temp / 10.0).round() * 10.0, a.temp);
    for v in [
        a.contrast,
        a.highlights,
        a.shadows,
        a.whites,
        a.blacks,
        a.tint,
        a.vibrance,
        a.clarity,
    ] {
        assert_eq!(v.round(), v);
    }
    // f32 -> JSON must not leak binary noise like -0.19999998807907104.
    let j = serde_json::to_string(&a).unwrap();
    assert!(!j.contains("99999") && !j.contains("00000"), "{j}");
}
