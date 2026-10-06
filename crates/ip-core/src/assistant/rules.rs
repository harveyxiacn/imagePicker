//! The rules engine (T0): Chinese and English command templates + entity extraction that turn a
//! sentence into assistant tool calls without any model.
//!
//! `parse` is a pure function of the message and a small context (people, presets, selection),
//! so it is table-tested. Everything it returns still goes through
//! [`super::tools::validate_call`] before it becomes a plan.

use std::sync::OnceLock;

use regex::{Captures, Regex};
use serde_json::{json, Map, Value};

use super::tools::{filter_query, validate_call};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    Zh,
    En,
}

impl Locale {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.to_ascii_lowercase();
        if s.starts_with("zh") {
            Some(Self::Zh)
        } else if s.starts_with("en") {
            Some(Self::En)
        } else {
            None
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            Self::Zh => "zh-CN",
            Self::En => "en",
        }
    }
}

/// A preset the user can name: its id and every name / alias it answers to.
#[derive(Debug, Clone)]
pub struct PresetName {
    pub id: String,
    pub names: Vec<String>,
}

pub struct RulesCtx<'a> {
    /// Named people (`id`, `name`).
    pub people: &'a [(i64, String)],
    pub presets: &'a [PresetName],
    /// Photos selected in the UI.
    pub selection: &'a [i64],
    pub current_photo_id: Option<i64>,
    pub locale: Locale,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub tool: String,
    pub args: Value,
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub calls: Vec<Call>,
    /// Why the message was not understood (with examples).
    pub unsupported: Option<String>,
    /// Remarks for the reply (an unknown place, an approximation).
    pub hints: Vec<String>,
}

/// Aliases of the built-in presets (ids as in `ip_render::builtin_presets`).
pub fn builtin_preset_names() -> Vec<PresetName> {
    let p = |id: &str, names: &[&str]| PresetName {
        id: id.to_string(),
        names: names.iter().map(|s| s.to_string()).collect(),
    };
    vec![
        p("natural", &["自然", "natural"]),
        p("vivid", &["鲜艳", "浓郁", "vivid", "vibrant"]),
        p(
            "film_warm",
            &[
                "暖调胶片",
                "胶片暖调",
                "暖色调",
                "暖调",
                "warm film",
                "film warm",
                "warm",
                "胶片",
            ],
        ),
        p(
            "film_cool",
            &[
                "冷调胶片",
                "胶片冷调",
                "冷色调",
                "冷调",
                "cool film",
                "film cool",
                "cool",
                "cold",
            ],
        ),
        p(
            "japanese_clean",
            &["日系清新", "日系", "小清新", "japanese clean", "japanese"],
        ),
        p("cinematic", &["电影感", "电影", "cinematic", "movie"]),
        p(
            "bw_classic",
            &[
                "经典黑白",
                "黑白",
                "bw classic",
                "classic b&w",
                "black and white",
                "b&w",
                "monochrome",
            ],
        ),
        p(
            "bw_contrast",
            &[
                "高对比黑白",
                "高反差黑白",
                "bw contrast",
                "high-contrast b&w",
                "high contrast black and white",
            ],
        ),
        p(
            "portrait_soft",
            &["柔和人像", "柔肤", "soft portrait", "portrait soft"],
        ),
        p(
            "landscape_pop",
            &["风景鲜明", "风景增强", "landscape pop"],
        ),
    ]
}

pub fn unsupported_help(l: Locale) -> String {
    match l {
        Locale::Zh => "我还不能理解这条指令。可以试试：「只看小明的4星以上照片」「每个场景只留2张，其他淘汰」「把闭眼的照片都淘汰」「所有照片接受AI评分」「一键修图」「消除路人」「导出精选到小红书尺寸」".to_string(),
        Locale::En => "I could not understand that. Try: \"only show Alice's photos rated 4 stars or more\", \"keep 2 per scene and reject the rest\", \"reject all photos with closed eyes\", \"accept AI ratings for all photos\", \"auto adjust\", \"remove bystanders\", \"export the picks for Instagram\"".to_string(),
    }
}

// ------------------------------------------------------------------ regexes

struct Rx {
    cn_unit: Regex,
    cn_top: Regex,
    en_unit: Regex,
    en_top: Regex,
    view: Regex,
    reject: Regex,
    unreject: Regex,
    pick: Regex,
    unpick: Regex,
    keep: Regex,
    clear_filter: Regex,
    describe: Regex,
    suggest: Regex,
    export: Regex,
    bystanders: Regex,
    besttake: Regex,
    profiles: Regex,
    auto_adjust: Regex,
    accept_ai: Regex,
    apply_verb: Regex,
    look_noun: Regex,
    rest: Regex,
    per_scene: Regex,
    per_group: Regex,
    set_rating_zh: Regex,
    set_rating_en: Regex,
    clear_rating: Regex,
    star_num: Regex,
    star_above: Regex,
    star_below: Regex,
    ai_word: Regex,
    flag_picked: Regex,
    flag_rejected: Regex,
    flag_not_rejected: Regex,
    flag_unflagged: Regex,
    label: Regex,
    edited: Regex,
    unedited: Regex,
    no_issues: Regex,
    has_issues: Regex,
    negation: Regex,
    issue: Vec<(&'static str, Regex)>,
    scene: Vec<(&'static str, Regex)>,
    scope_all: Regex,
    scope_selected: Regex,
    scope_current_photo: Regex,
    scope_current_filter: Regex,
    best_word: Regex,
    count: Vec<Regex>,
    smiling: Regex,
    eyes_open: Regex,
    looking: Regex,
    single: Regex,
    nobody: Regex,
    burst_best: Regex,
    sort_ai: Regex,
    sort_rating: Regex,
    sort_name: Regex,
    unknown_zh: Regex,
    unknown_en: Regex,
    or_word: Regex,
    dest: Regex,
    xhs: Regex,
    wechat: Regex,
    insta: Regex,
    original: Regex,
}

fn rx() -> &'static Rx {
    static RX: OnceLock<Rx> = OnceLock::new();
    RX.get_or_init(|| {
        let r = |p: &str| Regex::new(p).unwrap_or_else(|e| panic!("bad regex {p}: {e}"));
        let cn = "[零〇一二两三四五六七八九十百]+";
        let en_num = "one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve";
        Rx {
            cn_unit: r(&format!("({cn})\\s*(星|颗|张|幅|个|组|只|枚|照|份|名|次)")),
            cn_top: r(&format!("(前|最好的|最好|留|保留|只要|只留)\\s*({cn})")),
            en_unit: r(&format!(
                r"\b({en_num})\s+(stars?|photos?|shots?|frames?|pics?|pictures?|images?|best|each|per)\b"
            )),
            en_top: r(&format!(
                r"\b(top|first|best|keep|only|last|leave)\s+({en_num})\b"
            )),
            view: r(r"只看|只显示|只要|只留|看看|看下|看一下|显示|展示|筛选|过滤|找出|找一下|找|查看|列出|\b(show|display|filter|find|list|view|see|only|just|give me|look at)\b"),
            reject: r(r"淘汰|丢弃|拒绝|排除|删掉|删除|扔掉|弃掉|\b(reject|discard|cull|trash|delete|throw away|get rid of|bin)\b"),
            unreject: r(r"(取消|撤销|恢复|撤回).{0,3}淘汰|\b(unreject|un-reject|restore)\b|undo.{0,6}reject"),
            pick: r(r"(标记|标为|标成|设为|设置为|设成|改为)\s*(?:为|成)?\s*(选用|选中|精选|入选)|\b(mark|flag|set|label|tag)\b.{0,24}?\bas\s+(a\s+)?(pick(s|ed)?|keepers?|favou?rites?)\b|\bpick (them|these|all|it)\b"),
            unpick: r(r"(取消|清除|去掉|移除|撤销).{0,3}(选用|选中|旗标|标记)|\b(unflag|unpick|un-pick|clear (the )?flags?|remove (the )?flags?)\b"),
            keep: r(r"保留|留下|留|只要|\b(keep|leave|retain)\b"),
            clear_filter: r(r"(清除|取消|重置|去掉|清空)\s*(所有)?(筛选|过滤)|显示(全部|所有|全部照片|所有照片)|看全部|\b(clear|reset|remove)\b.{0,8}\bfilters?\b|\bshow (me )?(all|everything)\b|\bshow all\b"),
            describe: r(r"描述(一下)?(这张|这幅|该|当前)|(这张|这幅|当前).{0,6}(是什么|拍的什么|有什么)|\bdescribe\b|what('s| is) (in|on) (this|the) (photo|picture|image)|caption (this|the)"),
            suggest: r(r"修图建议|怎么修|如何(修|调)|怎样(修|调)|修改建议|调整建议|给.{0,4}建议|\bsuggest(ions)?\b.{0,20}\b(edit|adjust|retouch)|\bhow (should|can|do) i (edit|adjust|retouch|fix)\b|\bedit suggestions?\b"),
            export: r(r"导出|输出|\b(export|save out)\b"),
            bystanders: r(r"(消除|去除|去掉|移除|删除|抹掉|擦掉|清除|p掉).{0,4}路人|路人.{0,4}(消除|去除|去掉|移除|删除|抹掉|擦掉|清除)|\b(remove|erase|delete|clean up|get rid of)\b.{0,16}\b(bystanders?|passers?-?by|strangers?|tourists?|people in the background|background people)\b"),
            besttake: r(r"最佳表情|最佳合成|全员最佳|一键.{0,3}最佳|换脸最佳|\bbest[- ]?take\b|\bbest expressions?\b"),
            profiles: r(r"美颜档案|应用.{0,3}档案|人物档案|套用.{0,3}档案|\b(apply|use)\b.{0,12}\b(beauty |retouch(ing)? )?profiles?\b|\bbeauty profiles?\b"),
            auto_adjust: r(r"一键(修图|修片|美化|调整|调色|优化|增强)|自动(修图|修片|调色|调整|美化|优化)|智能(修图|调色|美化)|修图|\bauto[- ]?(adjust|enhance|edit|fix|correct|retouch)\b|\bone[- ]?click\b|\b(enhance|retouch|improve)\b"),
            accept_ai: r(r"(接受|采纳|采用|使用|应用|确认).{0,4}(ai|智能)\s*(评分|星级|评星|打分|分数)|(ai|智能)\s*(评分|星级|评星|打分).{0,4}(接受|采纳|采用|确认)|\baccept\b.{0,12}\bai\b|\b(use|adopt|take)\b.{0,6}\bai (rating|score)s?\b"),
            apply_verb: r(r"应用|套用|加上|加个|加|用|使用|换成|改成|改为|设为|调成|变成|转成|转为|做成|\b(apply|use|add|give|set|make|switch|change|turn|convert|put|grade)\b"),
            look_noun: r(r"滤镜|风格|调色|预设|色调|效果|\b(preset|filter|look|style|tone|grade|grading|effect)\b"),
            rest: r(r"其他|其余|剩下|剩余|别的|余下|其它|\b(the )?(rest|others?|remaining|remainder)\b"),
            per_scene: r(r"每个?\s*(场景|场次|地点|场合)|每一个?\s*(场景|场次)|\b(each|every|per)\s+(scene|location|place)\b"),
            per_group: r(r"每个?\s*(连拍|分组|组|系列|连拍组|burst)|\b(each|every|per)\s+(burst|group|series|sequence|stack)\b"),
            set_rating_zh: r(r"(打|评|评为|设为|设置为|设成|改为|改成|标为|标记为|标成|给.{0,24}?|星级设为)\s*(\d)\s*(?:颗)?星"),
            set_rating_en: r(r"\b(rate|set|give|mark|assign|make|rating)\b.{0,32}?(\d)[\s-]*stars?\b|\b(\d)[\s-]*stars? (to|for)\b|set .{0,12}rating (to )?(\d)"),
            clear_rating: r(r"(清除|取消|去掉|重置|清空|移除).{0,4}(评分|星级|评星|星)|\b(clear|remove|reset|unset)\b.{0,12}\b(ratings?|stars?)\b"),
            star_num: r(r"(\d)\s*(?:颗)?\s*星|(\d)[\s-]*stars?\b|\b(?:rated|rating)\s*(\d)\b"),
            star_above: r(r"(\d)\s*(?:颗)?\s*星\s*(以上|及以上|或以上|起|之上|往上|及以上的)|(至少|不低于|大于等于|≥|>=)\s*(\d)\s*(?:颗)?\s*星|(\d)[\s-]*stars?\s*(\+|or (more|higher|better|above)|and (up|above|over|higher|better)|plus|minimum)|\b(at least|minimum|min\.?|above|over)\s*(\d)[\s-]*stars?\b|(\d)\s*\+\s*stars?|\brated\s*(\d)\s*(\+|or (more|above|higher))"),
            star_below: r(r"(\d)\s*(?:颗)?\s*星\s*(以下|及以下|或以下|以内|之下|及以下的)|(最多|不超过|小于等于|≤|<=)\s*(\d)\s*(?:颗)?\s*星|(\d)[\s-]*stars?\s*(or (less|fewer|lower|worse|below)|and (below|under|lower))|\b(at most|under|below|less than)\s*(\d)[\s-]*stars?\b"),
            ai_word: r(r"(ai|智能)\s*(评分|星级|评星|打分|分数|评级)?\s*$|ai\s*(rated|rating|score)"),
            flag_picked: r(r"已选用|已选中|已入选|选用的|入选的|精选|\b(picked|picks|selects|favou?rites?|flagged|keepers?)\b"),
            flag_rejected: r(r"已淘汰|被淘汰|淘汰的|淘汰照片|淘汰了的|\b(rejected|rejects)\b"),
            flag_not_rejected: r(r"未淘汰|没被淘汰|没有被淘汰|非淘汰|没淘汰|\b(not rejected|unrejected|non-rejected)\b"),
            flag_unflagged: r(r"未标记|没标记|没有标记|未处理|\bunflagged\b"),
            label: r(r"(红|黄|绿|蓝|紫)\s*色?\s*(标签|标记|色标|标)|\b(red|yellow|green|blue|purple)\s*(label|tag|color label|colour label)\b"),
            edited: r(r"已修图|修过的?|修好的?|已编辑|编辑过的?|\b(edited|retouched)\b"),
            unedited: r(r"未修图|没修过|没有修过|未编辑|没编辑|\b(unedited|not edited|unretouched|untouched)\b"),
            no_issues: r(r"没有?问题|无问题|没毛病|没有缺陷|无缺陷|质量好的|\b(no|without|free of|any)\s+(issues?|problems?|flaws?|defects?)\b|\b(clean|flawless) (ones|photos|shots)\b"),
            has_issues: r(r"有问题|问题照片|有缺陷|有毛病|\b(with|have|has)\s+(issues?|problems?|flaws?|defects?)\b|problem(atic)? (photos|shots|ones)"),
            negation: r(r"(没有?|不|无|非|别|免|未)[\s-]*$|\b(without|no|not|free of|never|non)[\s-]*$"),
            issue: vec![
                ("closed_eyes", r(r"闭眼|眨眼|闭着眼|闭上眼|eyes? (are )?closed|closed[- ]eyes?|\bblink(ing|ed|s)?\b|eyes? shut")),
                ("blurry", r(r"模糊|虚焦|失焦|没对上焦|没对焦|糊的|糊了|\bblurr?(y|ed)?\b|out of focus|unsharp|soft focus|out-of-focus")),
                ("overexposed", r(r"过曝|曝光过度|太亮|过亮|\boverexpos\w*|\bblown[- ]?(out)?\b|too bright")),
                ("underexposed", r(r"欠曝|曝光不足|太暗|过暗|\bunderexpos\w*|too dark")),
                ("noisy", r(r"噪点|噪声|\bnois(y|e)\b|\bgrain(y)?\b")),
                ("tilted", r(r"倾斜|歪的|歪斜|没拍正|\btilted\b|\bcrooked\b|\bskewed\b|not level")),
            ],
            scene: vec![
                ("landscape", r(r"风景|风光|景色|\blandscapes?\b|\bscenery\b")),
                ("portrait", r(r"人像|肖像|\bportraits?\b")),
                ("group", r(r"合影|集体照|团体照|大合照|\bgroup (photos?|shots?|pictures?|portraits?)\b")),
                ("food", r(r"美食|食物|菜品|\bfood\b|\bmeals?\b|\bdishes\b")),
                ("architecture", r(r"建筑|\barchitecture\b|\bbuildings?\b")),
                ("night", r(r"夜景|夜晚|夜拍|夜间|\bnight( shots?| photos?| scenes?)?\b")),
                ("pet", r(r"宠物|猫咪|狗狗|小猫|小狗|\bpets?\b|\bcats?\b|\bdogs?\b|\bkittens?\b|\bpuppies\b")),
            ],
            scope_all: r(r"所有|全部|全都|一切|整个(相册|文件夹|会话)?|\bevery(thing| photo| picture| image)\b|\ball( of)?( the| my| these)?( photos?| pictures?| images?| shots?)?\b|\bthe whole (session|folder|album)\b"),
            scope_selected: r(r"(选中|选择|选定|勾选)的|所选|当前选中|选中这些|这些照片|这几张|这批|\bselected\b|\bselection\b|\bthese (photos?|ones|pictures?|images?|shots?)\b|\bthese\b|\bhighlighted\b"),
            scope_current_photo: r(r"这张|这一张|这幅|当前(这张)?照片|当前这张|当前的照片|\bthis (photo|picture|image|one|shot)\b|\bthe current (photo|one|image)\b|\bcurrent photo\b"),
            scope_current_filter: r(r"当前(筛选|过滤|列表|视图|结果)|筛选(出)?的|\b(current|active) (filter|view|results?|list)\b|\b(filtered|visible|shown|in view)\b"),
            best_word: r(r"最好|最佳|最高分|最棒|最优|\b(best|top|highest|finest|sharpest)\b"),
            count: vec![
                r(r"前\s*(\d+)\s*(?:张|个|幅|名|条)?"),
                r(r"(\d+)\s*(?:张|幅|个)(?:照片|图|相片)?"),
                r(r"(?:最好的?|最佳|留下?|保留|只要|只留)\s*(\d+)"),
                r(r"\b(?:top|first|best|keep|only|last|leave)\s+(\d+)\b"),
                r(r"\b(\d+)\s+(?:photos?|shots?|frames?|pics?|pictures?|images?|best|each|per)\b"),
            ],
            smiling: r(r"微笑|笑的|在笑|笑着|\bsmil(e|es|ing)\b"),
            eyes_open: r(r"睁眼|睁着眼|眼睛睁开|\beyes? open\b"),
            looking: r(r"看镜头|看向镜头|直视|\blooking at (the )?camera\b"),
            single: r(r"单人照|单人|独照|\bsingle (portrait|person)\b|\bsolo\b"),
            nobody: r(r"没有人|没人|无人|不含人|\b(no people|nobody|without people|empty)\b"),
            burst_best: r(r"(每组|每个连拍|连拍|每个分组)\s*(里)?\s*(的)?\s*(最佳|最好)|\b(best of (each|every) (burst|group)|burst (winners|bests)|best in (each|every) (burst|group))\b"),
            sort_ai: r(r"按\s*(ai|智能)\s*(评分|分数)?\s*排序|\bsort(ed)? by ai\b"),
            sort_rating: r(r"按\s*(评分|星级|星)\s*排序|\bsort(ed)? by (rating|stars)\b"),
            sort_name: r(r"按\s*(名称|文件名|名字)\s*排序|\bsort(ed)? by (name|filename)\b"),
            unknown_zh: r(r"([\p{Han}A-Za-z0-9]{1,10})的(?:照片|相片|图片|合影|图|片子)"),
            unknown_en: r(r"\bphotos? (?:of|from|in|at|taken in) ([A-Za-z][A-Za-z'’-]{1,24})\b|\b([A-Za-z][A-Za-z'’-]{1,24})['’]s (?:photos?|pictures?|pics|shots?)\b"),
            or_word: r(r"或者|或|还是|\bor\b"),
            dest: r(r"(?:到|至|进|入|\bto\b|\binto\b|\bin\b)\s*([a-zA-Z]:[\\/][^\s]*|/[^\s]+|~[\\/][^\s]*)"),
            xhs: r(r"小红书|xiaohongshu|red ?note|\bxhs\b"),
            wechat: r(r"微信|朋友圈|wechat|weixin|moments"),
            insta: r(r"instagram|\big\b|\bins\b|\binsta\b"),
            original: r(r"原图|原始|原尺寸|\boriginal(s)?\b|full[- ]?size"),
        }
    })
}



// ------------------------------------------------------------------ normalisation

fn cn_to_num(s: &str) -> Option<u32> {
    let digit = |c: char| match c {
        '零' | '〇' => Some(0),
        '一' => Some(1),
        '二' | '两' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        _ => None,
    };
    let (mut total, mut cur) = (0u32, 0u32);
    let mut any = false;
    for c in s.chars() {
        any = true;
        if let Some(d) = digit(c) {
            cur = d;
        } else if c == '十' {
            total += if cur == 0 { 10 } else { cur * 10 };
            cur = 0;
        } else if c == '百' {
            total += if cur == 0 { 100 } else { cur * 100 };
            cur = 0;
        } else {
            return None;
        }
    }
    any.then_some(total + cur)
}

fn en_to_num(s: &str) -> Option<u32> {
    Some(match s {
        "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        "eleven" => 11,
        "twelve" => 12,
        _ => return None,
    })
}

/// Lower case, full-width -> ASCII, CJK punctuation -> spaces, number words -> digits.
pub fn normalize(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        let c = match c {
            '\u{3000}' => ' ',
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(c as u32 - 0xfee0).unwrap_or(c),
            c => c,
        };
        match c {
            '，' | '。' | '！' | '？' | '、' | '；' | '：' | '（' | '）' | '「' | '」' | '『' | '』'
            | '“' | '”' | '《' | '》' | ',' | '!' | '?' | ';' | '(' | ')' | '"' | '[' | ']'
            | '{' | '}' => o.push(' '),
            '‘' | '’' => o.push('\''),
            c => o.extend(c.to_lowercase()),
        }
    }
    let x = rx();
    let o = x
        .cn_unit
        .replace_all(&o, |c: &Captures| match cn_to_num(&c[1]) {
            Some(n) => format!("{n}{}", &c[2]),
            None => c[0].to_string(),
        })
        .into_owned();
    let o = x
        .cn_top
        .replace_all(&o, |c: &Captures| match cn_to_num(&c[2]) {
            Some(n) => format!("{}{n}", &c[1]),
            None => c[0].to_string(),
        })
        .into_owned();
    let o = x
        .en_unit
        .replace_all(&o, |c: &Captures| match en_to_num(&c[1]) {
            Some(n) => format!("{n} {}", &c[2]),
            None => c[0].to_string(),
        })
        .into_owned();
    let o = x
        .en_top
        .replace_all(&o, |c: &Captures| match en_to_num(&c[2]) {
            Some(n) => format!("{} {n}", &c[1]),
            None => c[0].to_string(),
        })
        .into_owned();
    o.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

// ------------------------------------------------------------------ fuzzy matching

fn levenshtein(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Dice coefficient of the character multisets of two strings.
fn char_dice(a: &str, b: &str) -> f64 {
    let (mut ca, mut cb): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if ca.is_empty() || cb.is_empty() {
        return 0.0;
    }
    let total = (ca.len() + cb.len()) as f64;
    ca.sort_unstable();
    cb.sort_unstable();
    let (mut i, mut j, mut common) = (0, 0, 0);
    while i < ca.len() && j < cb.len() {
        match ca[i].cmp(&cb[j]) {
            std::cmp::Ordering::Equal => {
                common += 1;
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    2.0 * common as f64 / total
}

fn words(s: &str) -> Vec<&str> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '&' || c == '-'))
        .filter(|w| !w.is_empty())
        .collect()
}

/// Character span of `needle` in `hay` as char indices.
fn find_chars(hay: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == *needle)
}

fn blank_chars(t: &mut Vec<char>, start: usize, len: usize) {
    for c in t.iter_mut().skip(start).take(len) {
        *c = ' ';
    }
}

/// Matches people named in the text (exact, token or small typo); returns ids in order of
/// appearance and blanks the matched names out of `t`.
fn match_people(t: &mut Vec<char>, people: &[(i64, String)]) -> Vec<i64> {
    let mut found: Vec<(usize, i64)> = Vec::new();
    for (id, name) in people {
        let n: Vec<char> = normalize(name).chars().collect();
        if n.is_empty() || found.iter().any(|(_, i)| i == id) {
            continue;
        }
        if let Some(pos) = find_chars(t, &n) {
            found.push((pos, *id));
            blank_chars(t, pos, n.len());
            continue;
        }
        // Latin names: a word within one typo of the name, or of its first name
        let name_s: String = n.iter().collect();
        if name_s.chars().any(|c| c.is_ascii_alphabetic()) {
            let text: String = t.iter().collect();
            let mut hit = None;
            let mut offset = 0usize;
            for w in text.split(' ') {
                let wl = w.chars().count();
                if wl >= 3 {
                    let tokens: Vec<&str> = name_s.split(' ').collect();
                    let close = |cand: &str| {
                        cand.chars().count() >= 3
                            && levenshtein(w, cand) <= usize::from(cand.chars().count() >= 5)
                    };
                    if close(&name_s) || tokens.iter().any(|tok| tok.chars().count() >= 3 && close(tok))
                    {
                        hit = Some((offset, wl));
                        break;
                    }
                }
                offset += wl + 1;
            }
            if let Some((pos, len)) = hit {
                found.push((pos, *id));
                blank_chars(t, pos, len);
            }
        }
    }
    found.sort();
    found.into_iter().map(|(_, id)| id).collect()
}

/// Best preset for the text. `t` is blanked where the match was.
fn match_preset(t: &mut Vec<char>, presets: &[PresetName]) -> Option<(String, bool)> {
    let mut best: Option<(f64, String, usize, usize)> = None;
    let text: String = t.iter().collect();
    for p in presets {
        let mut names: Vec<String> = p.names.iter().map(|n| normalize(n)).collect();
        names.push(p.id.replace('_', " "));
        for name in names {
            if name.is_empty() {
                continue;
            }
            let nc: Vec<char> = name.chars().collect();
            let ascii = name.chars().all(|c| c.is_ascii());
            let mut score = 0.0;
            let mut span = (0, 0);
            if ascii {
                // whole words, in any order
                let ws = words(&text);
                let toks: Vec<&str> = name.split(' ').collect();
                if toks.iter().all(|k| ws.contains(k)) {
                    score = name.len() as f64 * 0.95;
                    if let Some(pos) = find_chars(t, &nc) {
                        span = (pos, nc.len());
                    } else {
                        // reordered ("film warm" vs "warm film"): blank each word
                        span = (0, 0);
                    }
                }
            } else if let Some(pos) = find_chars(t, &nc) {
                score = nc.len() as f64 + 0.5;
                span = (pos, nc.len());
            } else if nc.len() >= 4 {
                for w in nc.len().saturating_sub(1)..=nc.len() + 1 {
                    for start in 0..t.len().saturating_sub(w - 1) {
                        if start + w > t.len() {
                            break;
                        }
                        let win: String = t[start..start + w].iter().collect();
                        if win.chars().any(|c| c == ' ') {
                            continue;
                        }
                        let d = char_dice(&name, &win);
                        if d >= 0.75 && nc.len() as f64 * d > score {
                            score = nc.len() as f64 * d * 0.9;
                            span = (start, w);
                        }
                    }
                }
            }
            if score > 0.0 && best.as_ref().is_none_or(|b| score > b.0) {
                best = Some((score, p.id.clone(), span.0, span.1));
            }
        }
    }
    best.map(|(_, id, pos, len)| {
        blank_chars(t, pos, len);
        (id, true)
    })
}

// ------------------------------------------------------------------ extraction

#[derive(Debug, Default)]
struct Ents {
    persons: Vec<i64>,
    person_any: bool,
    rating_gte: Option<i64>,
    ai_rating_gte: Option<f64>,
    flag: Option<&'static str>,
    color_label: Option<&'static str>,
    has_edits: Option<bool>,
    issues: Vec<&'static str>,
    issues_none: bool,
    scene: Option<&'static str>,
    faces_min: Option<i64>,
    faces_max: Option<i64>,
    person_state: Vec<&'static str>,
    sort: Option<&'static str>,
    burst_best: bool,
    count: Option<i64>,
    best: bool,
    all: bool,
    selected: bool,
    current_photo: bool,
    current_filter: bool,
    rating_below: bool,
}

impl Ents {
    fn has_scope(&self) -> bool {
        !self.persons.is_empty()
            || self.rating_gte.is_some()
            || self.ai_rating_gte.is_some()
            || self.flag.is_some()
            || self.color_label.is_some()
            || self.has_edits.is_some()
            || !self.issues.is_empty()
            || self.issues_none
            || self.scene.is_some()
            || self.faces_min.is_some()
            || self.faces_max.is_some()
            || !self.person_state.is_empty()
            || self.burst_best
    }

    fn filter_args(&self) -> Value {
        let mut m = Map::new();
        if !self.persons.is_empty() {
            m.insert("persons".into(), json!(self.persons));
            if self.person_any {
                m.insert("person_mode".into(), json!("any"));
            }
        }
        if let Some(r) = self.rating_gte {
            m.insert("rating_gte".into(), json!(r));
        }
        if let Some(r) = self.ai_rating_gte {
            m.insert("ai_rating_gte".into(), json!(r));
        }
        if let Some(f) = self.flag {
            m.insert("flag".into(), json!(f));
        }
        if let Some(c) = self.color_label {
            m.insert("color_label".into(), json!(c));
        }
        if let Some(e) = self.has_edits {
            m.insert("has_edits".into(), json!(e));
        }
        if !self.issues.is_empty() {
            m.insert("issues_any".into(), json!(self.issues));
        }
        if self.issues_none {
            m.insert("issues_none".into(), json!(true));
        }
        if let Some(s) = self.scene {
            m.insert("scene_type".into(), json!(s));
        }
        if let Some(n) = self.faces_min {
            m.insert("faces_min".into(), json!(n));
        }
        if let Some(n) = self.faces_max {
            m.insert("faces_max".into(), json!(n));
        }
        if !self.person_state.is_empty() {
            m.insert("person_state".into(), json!(self.person_state));
        }
        if self.burst_best {
            m.insert("burst_best_only".into(), json!(true));
        }
        if let Some(s) = self.sort {
            m.insert("sort".into(), json!(s));
        }
        Value::Object(m)
    }
}

fn first_digit(c: &Captures) -> Option<i64> {
    (1..c.len())
        .filter_map(|i| c.get(i))
        .find_map(|m| m.as_str().parse::<i64>().ok())
}

fn extract(t: &str, ents: &mut Ents, view_or_no_action: bool) {
    let x = rx();
    // ---- ratings
    let mut ge = None;
    for c in x.star_above.captures_iter(t) {
        if let Some(n) = first_digit(&c) {
            ge = Some(n.clamp(0, 5));
        }
    }
    if x.star_below.is_match(t) {
        ents.rating_below = true;
    }
    if ge.is_none() && !ents.rating_below && view_or_no_action {
        // "5 stars" with a view verb / no verb: at least that many
        if let Some(c) = x.star_num.captures(t) {
            ge = first_digit(&c).map(|n| n.clamp(0, 5));
        }
    }
    // an "AI" word right before the star expression makes it the AI rating
    if let Some(n) = ge {
        let star_pos = x
            .star_above
            .find(t)
            .or_else(|| x.star_num.find(t))
            .map(|m| m.start())
            .unwrap_or(0);
        let before: String = t[..star_pos].chars().rev().take(8).collect::<String>().chars().rev().collect();
        if x.ai_word.is_match(before.trim_end()) || before.contains("ai") {
            ents.ai_rating_gte = Some(n as f64);
        } else {
            ents.rating_gte = Some(n);
        }
    }
    // ---- flags / labels / edits
    if x.flag_not_rejected.is_match(t) {
        ents.flag = Some("not_rejected");
    } else if x.flag_unflagged.is_match(t) {
        ents.flag = Some("unflagged");
    } else if x.flag_rejected.is_match(t) {
        ents.flag = Some("rejected");
    } else if x.flag_picked.is_match(t) {
        ents.flag = Some("picked");
    }
    if let Some(c) = x.label.captures(t) {
        let w = c.get(1).or_else(|| c.get(5)).map(|m| m.as_str());
        let w = w.or_else(|| {
            c.iter()
                .skip(1)
                .flatten()
                .map(|m| m.as_str())
                .find(|s| !s.is_empty())
        });
        ents.color_label = match w {
            Some("红" | "red") => Some("red"),
            Some("黄" | "yellow") => Some("yellow"),
            Some("绿" | "green") => Some("green"),
            Some("蓝" | "blue") => Some("blue"),
            Some("紫" | "purple") => Some("purple"),
            _ => None,
        };
    }
    if x.unedited.is_match(t) {
        ents.has_edits = Some(false);
    } else if x.edited.is_match(t) {
        ents.has_edits = Some(true);
    }
    // ---- issues
    if x.no_issues.is_match(t) {
        ents.issues_none = true;
    } else if x.has_issues.is_match(t) {
        ents.issues = vec![
            "closed_eyes",
            "blurry",
            "overexposed",
            "underexposed",
            "noisy",
            "tilted",
        ];
    }
    for (key, re) in &x.issue {
        for m in re.find_iter(t) {
            let before: String = t[..m.start()]
                .chars()
                .rev()
                .take(8)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            if x.negation.is_match(before.trim_end()) {
                ents.issues_none = true;
            } else if !ents.issues.contains(key) {
                ents.issues.push(key);
            }
        }
    }
    if !ents.issues.is_empty() {
        ents.issues_none = false;
    }
    // ---- scene / faces / states
    for (key, re) in &x.scene {
        if re.is_match(t) {
            ents.scene = Some(key);
            break;
        }
    }
    if x.single.is_match(t) {
        ents.faces_min = Some(1);
        ents.faces_max = Some(1);
    }
    if x.nobody.is_match(t) {
        ents.faces_max = Some(0);
    }
    if x.smiling.is_match(t) {
        ents.person_state.push("smiling");
    }
    if x.eyes_open.is_match(t) {
        ents.person_state.push("eyes_open");
    }
    if x.looking.is_match(t) {
        ents.person_state.push("looking");
    }
    if x.burst_best.is_match(t) {
        ents.burst_best = true;
    }
    if x.sort_ai.is_match(t) {
        ents.sort = Some("ai");
    } else if x.sort_rating.is_match(t) {
        ents.sort = Some("rating");
    } else if x.sort_name.is_match(t) {
        ents.sort = Some("name");
    }
    // ---- counts and scope words
    for re in &x.count {
        if let Some(c) = re.captures(t) {
            if let Some(n) = first_digit(&c) {
                if (1..=1000).contains(&n) {
                    ents.count = Some(n);
                    break;
                }
            }
        }
    }
    ents.best = x.best_word.is_match(t);
    ents.all = x.scope_all.is_match(t);
    ents.selected = x.scope_selected.is_match(t);
    ents.current_photo = x.scope_current_photo.is_match(t);
    ents.current_filter = x.scope_current_filter.is_match(t);
}

fn call(tool: &str, args: Value) -> Call {
    Call {
        tool: tool.to_string(),
        args,
    }
}

fn unsupported(l: Locale, reason: Option<String>) -> Parsed {
    let help = unsupported_help(l);
    Parsed {
        unsupported: Some(match reason {
            Some(r) => format!("{r} {help}"),
            None => help,
        }),
        ..Default::default()
    }
}

#[derive(Default)]
struct Acc {
    /// `filter` steps that precede the action.
    pre: Vec<Call>,
    hints: Vec<String>,
    err: Option<String>,
}

fn unknown_subject_msg(l: Locale, u: &str) -> String {
    match l {
        Locale::Zh => format!("找不到叫「{u}」的人物，也不支持按地点/事件筛选。可以先在人物页给人物命名。"),
        Locale::En => format!("There is no person named \"{u}\", and places/events cannot be filtered. Name the person in the People view first."),
    }
}

fn query_of(e: &Ents, limit: bool) -> String {
    let mut q = filter_query(&e.filter_args());
    if limit {
        if let Some(n) = e.count {
            if e.best && !q.contains("sort=") {
                if !q.is_empty() {
                    q.push('&');
                }
                q.push_str("sort=ai");
            }
            if !q.is_empty() {
                q.push('&');
            }
            q.push_str(&format!("limit={n}"));
        }
    }
    q
}

/// The selection an action applies to; a scope in the sentence also adds a `filter` step.
fn select(
    ctx: &RulesCtx,
    ents: &Ents,
    acc: &mut Acc,
    allow_count: bool,
    unknown: &Option<String>,
) -> Value {
    let l = ctx.locale;
    if ents.current_photo {
        return match ctx.current_photo_id {
            Some(id) => json!({"ids": [id]}),
            None => {
                acc.err = Some(match l {
                    Locale::Zh => "没有打开的照片，无法使用「这张」。".to_string(),
                    Locale::En => "No photo is open, so \"this photo\" has no target.".to_string(),
                });
                json!("current_filter")
            }
        };
    }
    if ents.selected {
        if ctx.selection.is_empty() {
            acc.err = Some(match l {
                Locale::Zh => "当前没有选中的照片。".to_string(),
                Locale::En => "No photos are selected.".to_string(),
            });
        }
        return json!({"ids": ctx.selection});
    }
    if ents.has_scope() || (allow_count && ents.count.is_some()) {
        let fa = ents.filter_args();
        if ents.has_scope() && fa.as_object().is_some_and(|m| !m.is_empty()) {
            acc.pre.push(call("filter", fa));
        }
        return json!({"query": query_of(ents, allow_count)});
    }
    if ents.current_filter {
        return json!("current_filter");
    }
    if ents.all {
        return json!({"query": ""});
    }
    if !ctx.selection.is_empty() {
        return json!({"ids": ctx.selection});
    }
    if let Some(u) = unknown {
        acc.hints.push(match l {
            Locale::Zh => format!("无法按「{u}」筛选（不是已命名的人物，也不支持按地点/事件筛选），已作用于当前筛选结果。"),
            Locale::En => format!("I cannot filter by \"{u}\" (not a named person, and places/events are not searchable); using the current filter instead."),
        });
    }
    json!("current_filter")
}

fn action(
    ctx: &RulesCtx,
    tool: &str,
    mut args: Map<String, Value>,
    ents: &Ents,
    acc: &mut Acc,
    unknown: &Option<String>,
    calls: &mut Vec<Call>,
) {
    let sel = select(ctx, ents, acc, true, unknown);
    args.insert("selection".into(), sel);
    calls.push(call(tool, Value::Object(args)));
}

/// Parses `msg` into tool calls (possibly none, with `unsupported` set).
pub fn parse(msg: &str, ctx: &RulesCtx) -> Parsed {
    let l = ctx.locale;
    let x = rx();
    let norm = normalize(msg);
    if norm.is_empty() {
        return unsupported(
            l,
            Some(match l {
                Locale::Zh => "消息是空的。".to_string(),
                Locale::En => "The message is empty.".to_string(),
            }),
        );
    }
    let mut t: Vec<char> = norm.chars().collect();

    // ---- clear filter
    if x.clear_filter.is_match(&norm) {
        return Parsed {
            calls: vec![call("filter", json!({}))],
            ..Default::default()
        };
    }

    // ---- intents that need no scope
    let is_describe = x.describe.is_match(&norm);
    let is_suggest = x.suggest.is_match(&norm);
    if is_describe || is_suggest {
        let tool = if is_suggest {
            "suggest_edits"
        } else {
            "describe"
        };
        return match ctx.current_photo_id {
            Some(id) => Parsed {
                calls: vec![call(tool, json!({"photo_id": id}))],
                ..Default::default()
            },
            None => unsupported(
                l,
                Some(match l {
                    Locale::Zh => "请先打开一张照片，我再来描述或给出建议。".to_string(),
                    Locale::En => "Open a photo first, then I can describe it or suggest edits.".to_string(),
                }),
            ),
        };
    }

    // ---- preset (blanked out of the text so its words are not read as scene words)
    let preset_allowed = x.apply_verb.is_match(&norm) || x.look_noun.is_match(&norm);
    let preset = if preset_allowed {
        match_preset(&mut t, ctx.presets).map(|p| p.0)
    } else {
        None
    };
    // "小红书" must not be read as the person 小红
    let xhs_hit = x.xhs.is_match(&norm);
    for m in x.xhs.find_iter(&norm) {
        let start = norm[..m.start()].chars().count();
        blank_chars(&mut t, start, m.as_str().chars().count());
    }
    // ---- people
    let mut persons = match_people(&mut t, ctx.people);
    persons.dedup();
    let text_no_names: String = t.iter().collect();

    let mut ents = Ents {
        persons,
        ..Default::default()
    };
    ents.person_any = ents.persons.len() > 1 && x.or_word.is_match(&text_no_names);

    // verbs are looked for in a text where flag-filter phrases ("淘汰的") are blanked
    let mut verb_text = text_no_names.clone();
    for re in [
        &x.flag_rejected,
        &x.flag_not_rejected,
        &x.flag_picked,
        &x.flag_unflagged,
    ] {
        verb_text = re.replace_all(&verb_text, " ").into_owned();
    }
    let view = x.view.is_match(&verb_text);
    let is_bystanders = x.bystanders.is_match(&verb_text);
    let is_besttake = x.besttake.is_match(&verb_text);
    let is_profiles = x.profiles.is_match(&verb_text);
    let is_accept_ai = x.accept_ai.is_match(&verb_text);
    let is_export = x.export.is_match(&verb_text);
    let per_scene = x.per_scene.is_match(&verb_text);
    let per_group = !per_scene && x.per_group.is_match(&verb_text);
    let unreject = x.unreject.is_match(&verb_text);
    let unpick = x.unpick.is_match(&verb_text);
    let pick = !unpick && x.pick.is_match(&text_no_names);
    let reject = !unreject && x.reject.is_match(&verb_text);
    let clear_rating = x.clear_rating.is_match(&verb_text);
    let set_rating_m = x
        .set_rating_zh
        .captures(&verb_text)
        .and_then(|c| first_digit(&c))
        .or_else(|| {
            x.set_rating_en
                .captures(&verb_text)
                .and_then(|c| first_digit(&c))
        });
    let keep = x.keep.is_match(&verb_text);
    let auto_adjust = x.auto_adjust.is_match(&verb_text);

    // ratings: a "set rating" phrase is never also a threshold
    let set_rating = if clear_rating {
        Some(None)
    } else {
        set_rating_m
            .filter(|_| !x.star_above.is_match(&verb_text))
            .map(|n| Some(n.clamp(0, 5)))
    };
    let action_present = is_bystanders
        || is_besttake
        || is_profiles
        || is_accept_ai
        || is_export
        || (per_scene || per_group) && (keep || reject || ents_count_hint(&verb_text))
        || unreject
        || unpick
        || pick
        || reject
        || set_rating.is_some()
        || preset.is_some()
        || auto_adjust;
    extract(&text_no_names, &mut ents, view || !action_present);
    if pick {
        // "mark as picks": the pick flag is the action, not a scope
        ents.flag = None;
    }
    if set_rating.is_some() {
        // the rating being set is not a filter
        ents.rating_gte = None;
        ents.ai_rating_gte = None;
    }
    if ents.rating_below {
        return unsupported(
            l,
            Some(match l {
                Locale::Zh => "暂不支持「N星以下」这样的筛选，请改用「N星以上」或旗标/问题条件。".to_string(),
                Locale::En => "Filtering by \"N stars or fewer\" is not supported yet; use \"N stars or more\", flags or issues instead.".to_string(),
            }),
        );
    }
    // an unknown "<X>的照片" subject: a place/event/person we know nothing about
    let mut unknown_subject: Option<String> = None;
    if ents.persons.is_empty() {
        let vocab = [
            "所有", "全部", "这些", "这张", "选中", "当前", "精选", "我", "这", "那", "你", "他", "她",
            "它", "该", "所选", "已", "未", "没", "有",
        ];
        let cand = x
            .unknown_zh
            .captures(&text_no_names)
            .map(|c| c[1].to_string())
            .or_else(|| {
                x.unknown_en
                    .captures(&text_no_names)
                    .and_then(|c| c.get(1).or_else(|| c.get(2)).map(|m| m.as_str().to_string()))
            });
        if let Some(c) = cand {
            // leading function words belong to the verb, not to the subject
            let c = c
                .trim_start_matches(|ch| "给对把将只看显示筛选找出查看让替用要的".contains(ch))
                .to_string();
            let known_word = x.scene.iter().any(|(_, r)| r.is_match(&c))
                || x.issue.iter().any(|(_, r)| r.is_match(&c))
                || x.flag_picked.is_match(&c)
                || x.flag_rejected.is_match(&c)
                || x.scope_all.is_match(&c)
                || x.scope_selected.is_match(&c)
                || x.edited.is_match(&c)
                || x.star_num.is_match(&c)
                || x.smiling.is_match(&c)
                || x.single.is_match(&c)
                || x.label.is_match(&c)
                || matches!(
                    c.as_str(),
                    "photo" | "photos" | "pictures" | "picture" | "the" | "these" | "my" | "all" | "of"
                        | "from" | "in" | "at" | "edited" | "selected" | "best" | "good" | "bad"
                )
                || vocab.iter().any(|v| c.starts_with(v))
                || c.is_empty()
                || c.chars().all(|ch| ch.is_ascii_digit());
            if !known_word {
                unknown_subject = Some(c);
            }
        }
    }

    // ---- selection of an action
    let mut acc = Acc::default();
    let un = &unknown_subject;
    let mut calls: Vec<Call> = Vec::new();

    // ---- intents, most specific first
    if is_export {
        let preset_id = if xhs_hit {
            Some("xiaohongshu")
        } else if x.wechat.is_match(&text_no_names) {
            Some("wechat")
        } else if x.insta.is_match(&text_no_names) {
            Some("instagram")
        } else if x.original.is_match(&text_no_names) {
            Some("original")
        } else {
            None
        };
        let mut extra = Map::new();
        if let Some(p) = preset_id {
            extra.insert("preset".into(), json!(p));
        }
        if let Some(d) = x.dest.captures(msg).and_then(|c| c.get(1)) {
            extra.insert("dest".into(), json!(d.as_str()));
        }
        action(ctx, "export", extra, &ents, &mut acc, un, &mut calls);
    } else if is_bystanders {
        action(ctx, "remove_bystanders", Map::new(), &ents, &mut acc, un, &mut calls);
    } else if is_besttake {
        action(ctx, "besttake_auto", Map::new(), &ents, &mut acc, un, &mut calls);
    } else if is_profiles {
        action(ctx, "apply_profiles", Map::new(), &ents, &mut acc, un, &mut calls);
    } else if is_accept_ai {
        action(ctx, "accept_ai", Map::new(), &ents, &mut acc, un, &mut calls);
    } else if (per_scene || per_group)
        && (keep || reject || ents.count.is_some())
        && !(view && !keep)
    {
        let rest = x.rest.is_match(&verb_text) && reject;
        let n = ents.count.unwrap_or(1).clamp(1, 100);
        let tool = if per_scene {
            "scene_keep_top"
        } else {
            "group_keep_top"
        };
        let mut args = Map::new();
        args.insert("n".into(), json!(n));
        args.insert("reject_rest".into(), json!(rest));
        if ents.has_scope() || ents.selected || ents.current_photo {
            let sel = select(ctx, &ents, &mut acc, false, un);
            args.insert("selection".into(), sel);
        } else if !ctx.selection.is_empty() && !ents.all {
            args.insert("selection".into(), json!({"ids": ctx.selection}));
        }
        calls.push(call(tool, Value::Object(args)));
    } else if unreject || unpick {
        let mut extra = Map::new();
        extra.insert("flag".into(), json!(0));
        action(ctx, "set_flag", extra, &ents, &mut acc, un, &mut calls);
    } else if let Some(r) = set_rating {
        let mut extra = Map::new();
        extra.insert("rating".into(), json!(r));
        action(ctx, "set_rating", extra, &ents, &mut acc, un, &mut calls);
    } else if let Some(p) = &preset {
        let mut extra = Map::new();
        extra.insert("preset_id".into(), json!(p));
        action(ctx, "apply_preset", extra, &ents, &mut acc, un, &mut calls);
    } else if auto_adjust && !view {
        let mode = match ents.scene {
            Some("portrait") | Some("group") => "portrait",
            Some("landscape") => "landscape",
            _ => "auto",
        };
        let mut extra = Map::new();
        extra.insert("mode".into(), json!(mode));
        action(ctx, "auto_adjust", extra, &ents, &mut acc, un, &mut calls);
    } else if reject && !view {
        let mut extra = Map::new();
        extra.insert("flag".into(), json!(-1));
        action(ctx, "set_flag", extra, &ents, &mut acc, un, &mut calls);
    } else if pick && !view {
        let mut extra = Map::new();
        extra.insert("flag".into(), json!(1));
        action(ctx, "set_flag", extra, &ents, &mut acc, un, &mut calls);
    } else if keep && !view && ents.has_scope() {
        let mut extra = Map::new();
        extra.insert("flag".into(), json!(1));
        action(ctx, "set_flag", extra, &ents, &mut acc, un, &mut calls);
    } else if ents.has_scope() || view || ents.sort.is_some() {
        if ents.has_scope() || ents.sort.is_some() {
            if let Some(u) = &unknown_subject {
                if !ents.has_scope() {
                    return unsupported(l, Some(unknown_subject_msg(l, u)));
                }
                acc.hints.push(match l {
                    Locale::Zh => format!("「{u}」无法作为筛选条件，已忽略。"),
                    Locale::En => format!("\"{u}\" cannot be used as a filter and was ignored."),
                });
            }
            if ents.count.is_some() {
                acc.hints.push(match l {
                    Locale::Zh => "筛选无法限制数量，已按条件显示全部匹配的照片。".to_string(),
                    Locale::En => {
                        "A filter cannot limit the number of photos; showing every match.".to_string()
                    }
                });
            }
            calls.push(call("filter", ents.filter_args()));
        } else if let Some(u) = &unknown_subject {
            return unsupported(l, Some(unknown_subject_msg(l, u)));
        } else {
            return unsupported(l, None);
        }
    } else {
        return unsupported(l, None);
    }
    let Acc {
        pre: pre_steps,
        hints,
        err: sel_error,
    } = acc;
    if let Some(e) = sel_error {
        return unsupported(l, Some(e));
    }
    let mut all = pre_steps;
    // a filter step that merely repeats the main one is dropped
    all.extend(calls);
    all.dedup_by(|b, a| a.tool == "filter" && b.tool == "filter" && a.args == b.args);
    // every call must be valid
    let mut out = Vec::new();
    for c in all {
        match validate_call(&c.tool, &c.args) {
            Ok(a) => out.push(Call {
                tool: c.tool,
                args: a,
            }),
            Err(e) => {
                return unsupported(
                    l,
                    Some(match l {
                        Locale::Zh => format!("无法生成有效的操作（{e}）。"),
                        Locale::En => format!("Could not build a valid action ({e})."),
                    }),
                )
            }
        }
    }
    let _ = has_cjk(msg);
    Parsed {
        calls: out,
        unsupported: None,
        hints,
    }
}

/// "每组 … 两张": a count next to a per-group word.
fn ents_count_hint(t: &str) -> bool {
    rx().count.iter().any(|r| r.is_match(t)) || rx().best_word.is_match(t)
}

#[cfg(test)]
mod tests;
