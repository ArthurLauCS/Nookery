//! 归类规则与配置文件。纯逻辑，不碰 Win32。

/// 规则键：目录
pub const FOLDER_KEY: &str = "/";
/// 规则键：没有专门规则的其他文件
pub const FALLBACK_KEY: &str = "*";

/// 还在写入中的临时文件
const TEMP_EXT: [&str; 5] = ["crdownload", "part", "partial", "download", "tmp"];

/// (中文名, 英文名, 规则键)
const DEFAULT_TILES: [(&str, &str, &str); 9] = [
    ("快捷方式", "Shortcuts", "lnk url appref-ms website"),
    ("文件夹", "Folders", "/"),
    ("文档", "Documents", "doc docx pdf txt md rtf odt xls xlsx csv ppt pptx wps et dps"),
    ("图片", "Pictures", "png jpg jpeg gif webp bmp svg heic ico psd"),
    ("视频", "Videos", "mp4 mkv mov avi wmv flv webm"),
    ("音频", "Music", "mp3 wav flac aac m4a ogg"),
    ("压缩包", "Archives", "zip rar 7z tar gz"),
    ("程序", "Programs", "exe msi"),
    ("其他", "Others", "*"),
];

pub struct TileCfg {
    pub name: String,
    /// 物理像素 x, y, w, h；w == 0 表示还没摆放过
    pub rect: [i32; 4],
    pub rolled: bool,
    /// 用户新建的磁贴，空了也显示
    pub keep: bool,
    /// 锁定后不能拖动、不能改尺寸
    pub locked: bool,
    /// 用户在磁贴里拖出来的顺序（文件名）；不在其中的项按默认顺序排在后面
    pub order: Vec<String>,
}

pub struct Config {
    pub tiles: Vec<TileCfg>,
    /// (扩展名或 FOLDER_KEY / FALLBACK_KEY, 磁贴名)
    pub rules: Vec<(String, String)>,
    /// 双击桌面空白处隐藏 / 显示磁贴
    pub dblclick: bool,
    /// 磁贴底色不透明度，百分比：100 / 75 / 50 / 25 / 0
    pub opacity: u8,
    /// 默认磁贴名当前是英文；旧配置没有这一项，都是中文
    pub en: bool,
    /// 用户拖进、改名或粘贴进某块磁贴的项 (桌面上的名字, 磁贴名)，优先于规则
    pub map: Vec<(String, String)>,
}

impl Config {
    /// 空文本得到默认配置
    pub fn parse(text: &str) -> Config {
        let mut cfg = Config { tiles: Vec::new(), rules: Vec::new(), dblclick: true, opacity: 100, en: false, map: Vec::new() };
        // 用记事本存成“带 BOM 的 UTF-8”时第一行前面多一个 BOM，不去掉第一项设置就读不出来
        for line in text.trim_start_matches('\u{feff}').lines() {
            let f: Vec<&str> = line.split('|').collect();
            match f.as_slice() {
                ["dblclick", v] => cfg.dblclick = *v == "1",
                ["map", name, tile] => cfg.map.push((name.to_string(), tile.to_string())),
                ["rule", key, tile] => cfg.rules.push((key.to_string(), tile.to_string())),
                ["opacity", v] => cfg.opacity = v.parse().unwrap_or(100),
                ["lang", v] => cfg.en = *v == "en",
                // 旧配置没有锁定和顺序这两段；顺序是锁定之后的若干列文件名（文件名不会含 |）
                ["tile", name, x, y, w, h, rolled, keep, rest @ ..] => cfg.tiles.push(TileCfg {
                    name: name.to_string(),
                    rect: [x, y, w, h].map(|v| v.parse().unwrap_or(0)),
                    rolled: *rolled == "1",
                    keep: *keep == "1",
                    locked: rest.first() == Some(&"1"),
                    order: rest.iter().skip(1).map(|s| s.to_string()).collect(),
                }),
                _ => {}
            }
        }
        if cfg.tiles.is_empty() && cfg.rules.is_empty() {
            for (name, _, keys) in DEFAULT_TILES {
                cfg.tiles.push(TileCfg { name: name.into(), rect: [0; 4], rolled: false, keep: false, locked: false, order: Vec::new() });
                cfg.rules.extend(keys.split(' ').map(|k| (k.to_string(), name.to_string())));
            }
        }
        cfg
    }

    pub fn to_text(&self) -> String {
        let lang = if self.en { "en" } else { "zh" };
        let mut s = format!("dblclick|{}\nopacity|{}\nlang|{lang}\n", self.dblclick as u8, self.opacity);
        for t in &self.tiles {
            let [x, y, w, h] = t.rect;
            s += &format!("tile|{}|{x}|{y}|{w}|{h}|{}|{}|{}", t.name, t.rolled as u8, t.keep as u8, t.locked as u8);
            for o in &t.order {
                s += &format!("|{o}");
            }
            s += "\n";
        }
        for (k, t) in &self.rules {
            s += &format!("rule|{k}|{t}\n");
        }
        for (n, t) in &self.map {
            s += &format!("map|{n}|{t}\n");
        }
        s
    }

    pub fn tile_for(&self, key: &str) -> Option<&str> {
        self.rules.iter().find(|(k, _)| k == key).map(|(_, t)| t.as_str())
    }

    /// 磁贴改名，指向它的规则和归属一起改
    pub fn rename_tile(&mut self, old: &str, new: &str) {
        for t in &mut self.tiles {
            if t.name == old {
                t.name = new.into();
            }
        }
        for (_, t) in self.rules.iter_mut().chain(&mut self.map) {
            if t == old {
                *t = new.into();
            }
        }
    }

    /// 切换到 en 语言时默认磁贴要改的名字 (旧名, 新名)；用户改过名的磁贴不在其中
    pub fn lang_renames(&self, en: bool) -> Vec<(&'static str, &'static str)> {
        if self.en == en {
            return Vec::new();
        }
        DEFAULT_TILES
            .iter()
            .map(|&(zh, e, _)| if en { (zh, e) } else { (e, zh) })
            .filter(|(from, _)| self.tiles.iter().any(|t| t.name == *from) || self.rules.iter().any(|(_, t)| t == from))
            .collect()
    }

    /// 改写（或新增）一条规则
    pub fn set_rule(&mut self, key: &str, tile: &str) {
        self.rules.retain(|(k, _)| k != key);
        self.rules.push((key.into(), tile.into()));
    }

    /// 把桌面上名为 name 的项固定放进 tile
    pub fn set_map(&mut self, name: &str, tile: &str) {
        self.map.retain(|(n, _)| n != name);
        self.map.push((name.into(), tile.into()));
    }

    /// 旧版快捷方式留在桌面、没有规则：补上“快捷方式”磁贴和规则
    pub fn add_shortcuts(&mut self) {
        if self.tile_for("lnk").is_some() {
            return;
        }
        let (zh, en, keys) = DEFAULT_TILES[0];
        let name = if self.en { en } else { zh };
        if self.tiles.iter().all(|t| t.name != name) {
            self.tiles.push(TileCfg { name: name.into(), rect: [0; 4], rolled: false, keep: false, locked: false, order: Vec::new() });
        }
        for k in keys.split(' ') {
            self.set_rule(k, name);
        }
    }
}

pub fn ext_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => name[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

/// 把 list 里属于 moved 的项（保持原有先后）挪到第 at 项前面；at 可以等于 list.len()（挪到最后）
pub fn reorder(list: &[String], moved: &[String], at: usize) -> Vec<String> {
    let at = at - list[..at].iter().filter(|n| moved.contains(n)).count();
    let mut v: Vec<String> = list.iter().filter(|n| !moved.contains(n)).cloned().collect();
    v.splice(at..at, list.iter().filter(|n| moved.contains(n)).cloned());
    v
}

/// 桌面上的一项显示在哪个磁贴；None 表示不显示（文件仍在桌面目录里）。
/// 此电脑、回收站等系统图标的名字是 ::{CLSID}，跟快捷方式走
pub fn classify<'a>(cfg: &'a Config, name: &str, is_dir: bool, hidden: bool) -> Option<&'a str> {
    if hidden || name.starts_with("~$") {
        return None;
    }
    if let Some((_, t)) = cfg.map.iter().find(|(n, _)| n == name) {
        return Some(t);
    }
    if is_dir {
        return cfg.tile_for(FOLDER_KEY);
    }
    let ext = if name.starts_with("::") { "lnk".into() } else { ext_of(name) };
    if TEMP_EXT.contains(&ext.as_str()) {
        return None;
    }
    cfg.tile_for(&ext).or_else(|| cfg.tile_for(FALLBACK_KEY))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_defaults() {
        let cfg = Config::parse("");
        assert_eq!(classify(&cfg, "微信.lnk", false, false), Some("快捷方式"));
        assert_eq!(classify(&cfg, "官网.URL", false, false), Some("快捷方式"));
        assert_eq!(classify(&cfg, "::{645FF040-5081-101B-9F08-00AA002F954E}", false, false), Some("快捷方式"));
        assert_eq!(classify(&cfg, "项目资料", true, false), Some("文件夹"));
        assert_eq!(classify(&cfg, "周报.DOCX", false, false), Some("文档"));
        assert_eq!(classify(&cfg, "a.tar.gz", false, false), Some("压缩包"));
        assert_eq!(classify(&cfg, "安装包.exe", false, false), Some("程序"));
        assert_eq!(classify(&cfg, "未知.xyz", false, false), Some("其他"));
        assert_eq!(classify(&cfg, "无扩展名", false, false), Some("其他"));
        assert_eq!(classify(&cfg, "电影.mp4.crdownload", false, false), None);
        assert_eq!(classify(&cfg, "~$周报.docx", false, false), None);
        assert_eq!(classify(&cfg, "desktop.ini", false, true), None);
        assert_eq!(classify(&cfg, "项目资料", true, true), None);
    }

    #[test]
    fn map_overrides_rules() {
        let mut cfg = Config::parse("");
        cfg.set_map("微信.lnk", "常用");
        cfg.set_map("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}", "常用");
        cfg.set_map("微信.lnk", "文档");
        assert_eq!(classify(&cfg, "微信.lnk", false, false), Some("文档"));
        cfg.rename_tile("常用", "系统");
        let back = Config::parse(&cfg.to_text());
        assert_eq!(classify(&back, "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}", false, false), Some("系统"));
        assert_eq!(back.map.len(), 2);
        // 隐藏后不显示，归属记录不影响
        assert_eq!(classify(&back, "微信.lnk", false, true), None);
    }

    #[test]
    fn old_config_gets_shortcuts() {
        let mut cfg = Config::parse("lang|en\ntile|Documents|1|2|3|4|0|0\nrule|docx|Documents\nrule|*|Others\npaused|1\nkeep|a.txt");
        assert_eq!(classify(&cfg, "a.lnk", false, false), Some("Others"));
        cfg.add_shortcuts();
        cfg.add_shortcuts();
        assert_eq!(classify(&cfg, "a.lnk", false, false), Some("Shortcuts"));
        assert_eq!(classify(&cfg, "a.website", false, false), Some("Shortcuts"));
        assert_eq!(cfg.tiles.iter().filter(|t| t.name == "Shortcuts").count(), 1);
    }

    #[test]
    fn rule_change_and_roundtrip() {
        let mut cfg = Config::parse("");
        cfg.set_rule("pdf", "图片");
        cfg.tiles[1].rect = [10, 20, 300, 200];
        cfg.tiles[1].rolled = true;
        cfg.dblclick = false;
        cfg.tiles[1].locked = true;
        cfg.opacity = 25;
        let back = Config::parse(&cfg.to_text());
        assert_eq!(classify(&back, "说明.pdf", false, false), Some("图片"));
        assert_eq!(back.tiles[1].rect, [10, 20, 300, 200]);
        assert!(back.tiles[1].rolled && !back.dblclick);
        assert!(Config::parse("").dblclick);
        assert!(back.tiles[1].locked && !back.tiles[0].locked);
        assert_eq!((back.opacity, Config::parse("").opacity), (25, 100));
        // 旧格式（没有锁定列）照样能读
        assert!(!Config::parse("tile|文档|1|2|3|4|0|0").tiles[0].locked);
        assert_eq!(Config::parse("\u{feff}opacity|50").opacity, 50);
        assert_eq!(back.tiles.len(), 9);
        cfg.tiles[2].order = vec!["b.png".into(), "a b.png".into()];
        assert_eq!(Config::parse(&cfg.to_text()).tiles[2].order, ["b.png", "a b.png"]);
        assert!(back.tiles[1].order.is_empty());
    }

    #[test]
    fn reorder_moves_before_slot() {
        let l: Vec<String> = ["a", "b", "c", "d"].map(String::from).to_vec();
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(reorder(&l, &s(&["a"]), 3), s(&["b", "c", "a", "d"]));
        assert_eq!(reorder(&l, &s(&["d"]), 0), s(&["d", "a", "b", "c"]));
        assert_eq!(reorder(&l, &s(&["b", "d"]), 4), s(&["a", "c", "b", "d"]));
        assert_eq!(reorder(&l, &s(&["a", "c"]), 2), s(&["b", "a", "c", "d"]));
    }

    #[test]
    fn lang_switch_keeps_custom_names() {
        let mut cfg = Config::parse("");
        cfg.rename_tile("图片", "截图");
        let renames = cfg.lang_renames(true);
        assert!(renames.contains(&("文档", "Documents")) && !renames.iter().any(|(f, _)| *f == "图片"));
        for (from, to) in renames {
            cfg.rename_tile(from, to);
        }
        cfg.en = true;
        assert_eq!(classify(&cfg, "周报.docx", false, false), Some("Documents"));
        assert_eq!(classify(&cfg, "a.png", false, false), Some("截图"));
        let back = Config::parse(&cfg.to_text());
        assert!(back.en && back.lang_renames(true).is_empty());
        assert!(back.lang_renames(false).contains(&("Documents", "文档")));
        assert!(!Config::parse("tile|文档|1|2|3|4|0|0").en);
    }

    #[test]
    fn deleted_fallback_hides_unknown() {
        let mut cfg = Config::parse("");
        cfg.rules.retain(|(_, t)| t != "其他");
        assert_eq!(classify(&cfg, "未知.xyz", false, false), None);
        assert_eq!(classify(&cfg, "周报.docx", false, false), Some("文档"));
    }
}
