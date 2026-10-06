//! Table tests of the rules engine: utterance -> tool calls.

use serde_json::{json, Value};

use super::*;

const CURRENT: i64 = 99;

fn people() -> Vec<(i64, String)> {
    vec![(1, "小明".into()), (2, "小红".into()), (3, "Alice".into())]
}

fn presets() -> Vec<PresetName> {
    let mut p = builtin_preset_names();
    p.push(PresetName {
        id: "user_7".into(),
        names: vec!["我的复古".into()],
    });
    p
}

fn ask_with(msg: &str, selection: &[i64], current: Option<i64>) -> Parsed {
    let people = people();
    let presets = presets();
    parse(
        msg,
        &RulesCtx {
            people: &people,
            presets: &presets,
            selection,
            current_photo_id: current,
            locale: if has_cjk(msg) { Locale::Zh } else { Locale::En },
        },
    )
}

fn ask(msg: &str) -> Parsed {
    ask_with(msg, &[], Some(CURRENT))
}

/// `have` contains every key of `want` with an equal value (objects recursively).
fn subset(want: &Value, have: &Value) -> bool {
    match (want, have) {
        (Value::Object(w), Value::Object(h)) => w
            .iter()
            .all(|(k, v)| h.get(k).is_some_and(|hv| subset(v, hv))),
        (w, h) => w == h,
    }
}

#[track_caller]
fn check(msg: &str, want: &[(&str, Value)]) {
    check_with(msg, &[], Some(CURRENT), want);
}

#[track_caller]
fn check_with(msg: &str, selection: &[i64], current: Option<i64>, want: &[(&str, Value)]) {
    let p = ask_with(msg, selection, current);
    assert!(
        p.unsupported.is_none(),
        "{msg:?} was not understood: {:?}",
        p.unsupported
    );
    let got: Vec<String> = p
        .calls
        .iter()
        .map(|c| format!("{} {}", c.tool, c.args))
        .collect();
    assert_eq!(
        p.calls.len(),
        want.len(),
        "{msg:?}: expected {} steps, got {got:?}",
        want.len()
    );
    for (c, (tool, args)) in p.calls.iter().zip(want) {
        assert_eq!(c.tool, *tool, "{msg:?}: {got:?}");
        assert!(
            subset(args, &c.args),
            "{msg:?}: step {tool} should contain {args}, got {}",
            c.args
        );
        // everything the engine returns is valid
        assert!(
            validate_call(&c.tool, &c.args).is_ok(),
            "{msg:?}: invalid {:?}",
            c.args
        );
    }
}

#[track_caller]
fn unsupported(msg: &str) -> String {
    let p = ask(msg);
    assert!(
        p.calls.is_empty(),
        "{msg:?} should not produce steps: {:?}",
        p.calls
    );
    p.unsupported
        .unwrap_or_else(|| panic!("{msg:?} should be unsupported"))
}

#[test]
fn chinese_filters() {
    check(
        "只看小明的4星以上照片",
        &[("filter", json!({"persons":[1],"rating_gte":4}))],
    );
    check(
        "只看风景照",
        &[("filter", json!({"scene_type":"landscape"}))],
    );
    check(
        "只看模糊的照片",
        &[("filter", json!({"issues_any":["blurry"]}))],
    );
    check(
        "看看过曝的",
        &[("filter", json!({"issues_any":["overexposed"]}))],
    );
    check(
        "显示已淘汰的照片",
        &[("filter", json!({"flag":"rejected"}))],
    );
    check(
        "只看小明和小红的合影",
        &[("filter", json!({"persons":[1,2],"scene_type":"group"}))],
    );
    check(
        "小明或小红的照片",
        &[("filter", json!({"persons":[1,2],"person_mode":"any"}))],
    );
    check(
        "只看没有问题的照片",
        &[("filter", json!({"issues_none":true}))],
    );
    check("只看5星", &[("filter", json!({"rating_gte":5}))]);
    check(
        "AI评分4星以上的照片",
        &[("filter", json!({"ai_rating_gte":4.0}))],
    );
    check(
        "只看笑的照片",
        &[("filter", json!({"person_state":["smiling"]}))],
    );
    check("按AI评分排序", &[("filter", json!({"sort":"ai"}))]);
    check("只看已修图的", &[("filter", json!({"has_edits":true}))]);
    check(
        "只看红色标签的照片",
        &[("filter", json!({"color_label":"red"}))],
    );
    check("清除筛选", &[("filter", json!({}))]);
    check("显示全部", &[("filter", json!({}))]);
    check("只看夜景", &[("filter", json!({"scene_type":"night"}))]);
}

#[test]
fn chinese_actions() {
    check(
        "每个场景只留2张，其他淘汰",
        &[("scene_keep_top", json!({"n":2,"reject_rest":true}))],
    );
    check(
        "每组保留最好的一张",
        &[("group_keep_top", json!({"n":1,"reject_rest":false}))],
    );
    check(
        "每个连拍组留前两张其余淘汰",
        &[("group_keep_top", json!({"n":2,"reject_rest":true}))],
    );
    check(
        "把闭眼的照片都淘汰",
        &[
            ("filter", json!({"issues_any":["closed_eyes"]})),
            (
                "set_flag",
                json!({"flag":-1,"selection":{"query":"issues_any=closed_eyes"}}),
            ),
        ],
    );
    check(
        "删掉所有模糊的照片",
        &[
            ("filter", json!({"issues_any":["blurry"]})),
            ("set_flag", json!({"flag":-1})),
        ],
    );
    check(
        "所有照片接受AI评分",
        &[("accept_ai", json!({"selection":{"query":""}}))],
    );
    check(
        "给京都的照片应用胶片暖调",
        &[(
            "apply_preset",
            json!({"preset_id":"film_warm","selection":"current_filter"}),
        )],
    );
    check(
        "导出精选到小红书尺寸",
        &[
            ("filter", json!({"flag":"picked"})),
            (
                "export",
                json!({"preset":"xiaohongshu","selection":{"query":"flag=picked"}}),
            ),
        ],
    );
    check(
        "一键修图",
        &[(
            "auto_adjust",
            json!({"mode":"auto","selection":"current_filter"}),
        )],
    );
    check(
        "对人像照片一键修图",
        &[
            ("filter", json!({"scene_type":"portrait"})),
            ("auto_adjust", json!({"mode":"portrait"})),
        ],
    );
    check(
        "消除路人",
        &[("remove_bystanders", json!({"selection":"current_filter"}))],
    );
    check("应用美颜档案", &[("apply_profiles", json!({}))]);
    check("最佳表情", &[("besttake_auto", json!({}))]);
    check(
        "把前3张评5星",
        &[(
            "set_rating",
            json!({"rating":5,"selection":{"query":"limit=3"}}),
        )],
    );
    check(
        "给小明的照片打5星",
        &[
            ("filter", json!({"persons":[1]})),
            ("set_rating", json!({"rating":5})),
        ],
    );
    check("清除评分", &[("set_rating", json!({"rating":null}))]);
    check("取消淘汰", &[("set_flag", json!({"flag":0}))]);
    check(
        "给小红的照片加电影感滤镜",
        &[
            ("filter", json!({"persons":[2]})),
            ("apply_preset", json!({"preset_id":"cinematic"})),
        ],
    );
    check(
        "用黑白风格处理所有风景照",
        &[
            ("filter", json!({"scene_type":"landscape"})),
            ("apply_preset", json!({"preset_id":"bw_classic"})),
        ],
    );
    check(
        "给风景照应用暖色胶片",
        &[
            ("filter", json!({"scene_type":"landscape"})),
            ("apply_preset", json!({"preset_id":"film_warm"})),
        ],
    );
    check(
        "把红色标签的照片导出到微信",
        &[
            ("filter", json!({"color_label":"red"})),
            ("export", json!({"preset":"wechat"})),
        ],
    );
    check(
        "导出4星以上的照片到 D:\\out 用instagram尺寸",
        &[
            ("filter", json!({"rating_gte":4})),
            (
                "export",
                json!({"preset":"instagram","dest":"D:\\out","selection":{"query":"rating_gte=4"}}),
            ),
        ],
    );
    check(
        "应用我的复古滤镜",
        &[("apply_preset", json!({"preset_id":"user_7"}))],
    );
}

#[test]
fn english_commands() {
    check(
        "only show Alice's photos rated 4 stars or more",
        &[("filter", json!({"persons":[3],"rating_gte":4}))],
    );
    check(
        "keep 2 per scene and reject the rest",
        &[("scene_keep_top", json!({"n":2,"reject_rest":true}))],
    );
    check(
        "keep the best one in each burst",
        &[("group_keep_top", json!({"n":1,"reject_rest":false}))],
    );
    check(
        "reject all photos with closed eyes",
        &[
            ("filter", json!({"issues_any":["closed_eyes"]})),
            ("set_flag", json!({"flag":-1})),
        ],
    );
    check(
        "accept AI ratings for all photos",
        &[("accept_ai", json!({"selection":{"query":""}}))],
    );
    check("auto adjust", &[("auto_adjust", json!({"mode":"auto"}))]);
    check("remove bystanders", &[("remove_bystanders", json!({}))]);
    check(
        "export the picks for Instagram",
        &[
            ("filter", json!({"flag":"picked"})),
            ("export", json!({"preset":"instagram"})),
        ],
    );
    check(
        "show me only 5 star photos",
        &[("filter", json!({"rating_gte":5}))],
    );
    check(
        "apply the warm film look to landscapes",
        &[
            ("filter", json!({"scene_type":"landscape"})),
            ("apply_preset", json!({"preset_id":"film_warm"})),
        ],
    );
    check(
        "rate the first three photos 5 stars",
        &[(
            "set_rating",
            json!({"rating":5,"selection":{"query":"limit=3"}}),
        )],
    );
    check("clear the filter", &[("filter", json!({}))]);
    check(
        "show only edited photos",
        &[("filter", json!({"has_edits":true}))],
    );
    check(
        "show photos without blur",
        &[("filter", json!({"issues_none":true}))],
    );
    check(
        "only show alise photos",
        &[("filter", json!({"persons":[3]}))],
    );
    check(
        "Rate Alice's photos 3 stars",
        &[
            ("filter", json!({"persons":[3]})),
            ("set_rating", json!({"rating":3})),
        ],
    );
    check(
        "show food photos",
        &[("filter", json!({"scene_type":"food"}))],
    );
    check(
        "use the vivid preset",
        &[("apply_preset", json!({"preset_id":"vivid"}))],
    );
}

#[test]
fn selection_and_current_photo() {
    check_with(
        "选中的照片全部淘汰",
        &[5, 6],
        Some(CURRENT),
        &[("set_flag", json!({"flag":-1,"selection":{"ids":[5,6]}}))],
    );
    check_with(
        "mark these as picks",
        &[1, 2],
        None,
        &[("set_flag", json!({"flag":1,"selection":{"ids":[1,2]}}))],
    );
    check_with(
        "把这张标记为选用",
        &[],
        Some(CURRENT),
        &[("set_flag", json!({"flag":1,"selection":{"ids":[CURRENT]}}))],
    );
    // an action with a selection and no other scope uses the selection
    check_with(
        "一键修图",
        &[7, 8, 9],
        None,
        &[("auto_adjust", json!({"selection":{"ids":[7,8,9]}}))],
    );
    check("描述这张照片", &[("describe", json!({"photo_id":CURRENT}))]);
    check(
        "describe this photo",
        &[("describe", json!({"photo_id":CURRENT}))],
    );
    check(
        "给我一些修图建议",
        &[("suggest_edits", json!({"photo_id":CURRENT}))],
    );
    // missing context
    let p = ask_with("选中的照片全部淘汰", &[], None);
    assert!(p.unsupported.is_some() && p.calls.is_empty());
    let p = ask_with("描述这张照片", &[], None);
    assert!(p.unsupported.is_some());
}

#[test]
fn unsupported_messages_help() {
    for msg in [
        "",
        "   ",
        "你好",
        "make the sky pink",
        "what's wrong with the weather",
        "把3星以下的淘汰",
        "show photos with 2 stars or less",
        "只看京都的照片",
        "只看小李的照片",
    ] {
        let why = unsupported(msg);
        assert!(
            why.contains("只看") || why.contains("show"),
            "{msg:?}: {why}"
        );
    }
    assert!(unsupported("只看京都的照片").contains("京都"));
    assert!(unsupported("只看小李的照片").contains("小李"));
    assert!(unsupported("把3星以下的淘汰").contains("以下"));
}

#[test]
fn unknown_place_is_a_hint_not_a_filter() {
    let p = ask("给京都的照片应用胶片暖调");
    assert!(p.unsupported.is_none());
    assert!(
        p.hints.iter().any(|h| h.contains("京都")),
        "the place is mentioned: {:?}",
        p.hints
    );
}

#[test]
fn numbers_in_both_languages() {
    assert_eq!(normalize("每个场景留两张"), "每个场景留2张");
    assert_eq!(normalize("前三张"), "前3张");
    assert_eq!(normalize("留下十二张"), "留下12张");
    assert_eq!(normalize("二十三星"), "23星");
    assert_eq!(normalize("一键修图"), "一键修图", "no unit, no number");
    assert_eq!(normalize("keep two per scene"), "keep 2 per scene");
    assert_eq!(normalize("one-click fix"), "one-click fix");
    assert_eq!(normalize("给我看４星以上的照片！"), "给我看4星以上的照片");
    assert_eq!(cn_to_num("一百零五"), Some(105));
    assert_eq!(cn_to_num("十"), Some(10));
    assert_eq!(cn_to_num("abc"), None);
}

#[test]
fn people_names_match_fuzzily() {
    let mut t: Vec<char> = "photos of alise and 小明".chars().collect();
    let found = match_people(&mut t, &people());
    assert_eq!(found, [3, 1], "in order of appearance");
    let s: String = t.iter().collect();
    assert!(
        !s.contains("小明") && !s.contains("alise"),
        "names are blanked: {s}"
    );
    let mut t: Vec<char> = "someone else".chars().collect();
    assert!(match_people(&mut t, &people()).is_empty());
    // a short word one typo away from a short name is not enough
    let mut t: Vec<char> = "ali".chars().collect();
    assert!(match_people(&mut t, &people()).is_empty());
}

#[test]
fn preset_names_match_fuzzily() {
    let presets = presets();
    let mut t: Vec<char> = "应用冷调胶片".chars().collect();
    assert_eq!(match_preset(&mut t, &presets).unwrap().0, "film_cool");
    let mut t: Vec<char> = "胶片冷调".chars().collect();
    assert_eq!(match_preset(&mut t, &presets).unwrap().0, "film_cool");
    let mut t: Vec<char> = "cinematic look".chars().collect();
    assert_eq!(match_preset(&mut t, &presets).unwrap().0, "cinematic");
    let mut t: Vec<char> = "高对比黑白".chars().collect();
    assert_eq!(match_preset(&mut t, &presets).unwrap().0, "bw_contrast");
    let mut t: Vec<char> = "今天天气不错".chars().collect();
    assert!(match_preset(&mut t, &presets).is_none());
}

#[test]
fn every_table_result_passes_the_validator() {
    for msg in [
        "只看小明的4星以上照片",
        "每个场景只留2张，其他淘汰",
        "导出精选到小红书尺寸",
        "keep 3 per group",
    ] {
        let p = ask(msg);
        for c in &p.calls {
            assert!(validate_call(&c.tool, &c.args).is_ok(), "{msg}: {c:?}");
        }
    }
}
