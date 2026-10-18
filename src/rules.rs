//! 归类规则与配置文件。纯逻辑，不碰 Win32。

/// 规则键：目录
pub const FOLDER_KEY: &str = "/";
/// 规则键：没有专门规则的其他文件
pub const FALLBACK_KEY: &str = "*";

/// 快捷方式留在桌面，不归类
const SHORTCUT_EXT: [&str; 4] = ["lnk", "url", "appref-ms", "website"];
/// 还在写入中的临时文件
const TEMP_EXT: [&str; 5] = ["crdownload", "part", "partial", "download", "tmp"];

/// (中文名, 英文名, 规则键)
const DEFAULT_TILES: [(&str, &str, &str); 8] = [
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
}

pub struct Config {
    pub tiles: Vec<TileCfg>,
    /// (扩展名或 FOLDER_KEY / FALLBACK_KEY, 磁贴名)
    pub rules: Vec<(String, String)>,
    pub paused: bool,
    /// 双击桌面空白处隐藏 / 显示磁贴
    pub dblclick: bool,
    /// 用户从磁贴拖回桌面的项，留在桌面不再收回
    pub keep: Vec<String>,
    /// 磁贴底色不透明度，百分比：100 / 75 / 50 / 25 / 0
    pub opacity: u8,
    /// 默认磁贴名当前是英文；旧配置没有这一项，都是中文
    pub en: bool,
}

impl Config {
    /// 空文本得到默认配置
    pub fn parse(text: &str) -> Config {
        let mut cfg = Config { tiles: Vec::new(), rules: Vec::new(), paused: false, dblclick: true, keep: Vec::new(), opacity: 100, en: false };
        // 用记事本存成“带 BOM 的 UTF-8”时第一行前面多一个 BOM，不去掉第一项设置就读不出来
        for line in text.trim_start_matches('\u{feff}').lines() {
            let f: Vec<&str> = line.split('|').collect();
            match f.as_slice() {
                ["paused", v] => cfg.paused = *v == "1",
                ["dblclick", v] => cfg.dblclick = *v == "1",
                ["keep", name] => cfg.keep.push(name.to_string()),
                ["rule", key, tile] => cfg.rules.push((key.to_string(), tile.to_string())),
                ["opacity", v] => cfg.opacity = v.parse().unwrap_or(100),
                ["lang", v] => cfg.en = *v == "en",
                // 旧配置没有锁定这一列
                ["tile", name, x, y, w, h, rolled, keep, locked @ ..] => cfg.tiles.push(TileCfg {
                    name: name.to_string(),
                    rect: [x, y, w, h].map(|v| v.parse().unwrap_or(0)),
                    rolled: *rolled == "1",
                    keep: *keep == "1",
                    locked: locked.first() == Some(&"1"),
                }),
                _ => {}
            }
        }
        if cfg.tiles.is_empty() && cfg.rules.is_empty() {
            for (name, _, keys) in DEFAULT_TILES {
                cfg.tiles.push(TileCfg { name: name.into(), rect: [0; 4], rolled: false, keep: false, locked: false });
                cfg.rules.extend(keys.split(' ').map(|k| (k.to_string(), name.to_string())));
            }
        }
        cfg
    }

    pub fn to_text(&self) -> String {
        let lang = if self.en { "en" } else { "zh" };
        let mut s = format!("paused|{}\ndblclick|{}\nopacity|{}\nlang|{lang}\n", self.paused as u8, self.dblclick as u8, self.opacity);
        for t in &self.tiles {
            let [x, y, w, h] = t.rect;
            s += &format!("tile|{}|{x}|{y}|{w}|{h}|{}|{}|{}\n", t.name, t.rolled as u8, t.keep as u8, t.locked as u8);
        }
        for (k, t) in &self.rules {
            s += &format!("rule|{k}|{t}\n");
        }
        for k in &self.keep {
            s += &format!("keep|{k}\n");
        }
        s
    }

    pub fn tile_for(&self, key: &str) -> Option<&str> {
        self.rules.iter().find(|(k, _)| k == key).map(|(_, t)| t.as_str())
    }

    /// 磁贴改名，指向它的规则一起改
    pub fn rename_tile(&mut self, old: &str, new: &str) {
        for t in &mut self.tiles {
            if t.name == old {
                t.name = new.into();
            }
        }
        for (_, t) in &mut self.rules {
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
}

pub fn ext_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => name[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

/// 桌面根目录上的一项该归入哪个磁贴；None 表示留在桌面
pub fn classify<'a>(cfg: &'a Config, name: &str, is_dir: bool, hidden: bool) -> Option<&'a str> {
    if hidden || name.starts_with("~$") {
        return None;
    }
    if is_dir {
        return cfg.tile_for(FOLDER_KEY);
    }
    let ext = ext_of(name);
    if SHORTCUT_EXT.contains(&ext.as_str()) || TEMP_EXT.contains(&ext.as_str()) {
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
        assert_eq!(classify(&cfg, "微信.lnk", false, false), None);
        assert_eq!(classify(&cfg, "官网.URL", false, false), None);
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
    fn rule_change_and_roundtrip() {
        let mut cfg = Config::parse("");
        cfg.set_rule("pdf", "图片");
        cfg.tiles[1].rect = [10, 20, 300, 200];
        cfg.tiles[1].rolled = true;
        cfg.paused = true;
        cfg.dblclick = false;
        cfg.keep.push("放在桌面.docx".into());
        cfg.tiles[1].locked = true;
        cfg.opacity = 25;
        let back = Config::parse(&cfg.to_text());
        assert_eq!(classify(&back, "说明.pdf", false, false), Some("图片"));
        assert_eq!(back.tiles[1].rect, [10, 20, 300, 200]);
        assert!(back.tiles[1].rolled && back.paused && !back.dblclick);
        assert!(Config::parse("").dblclick);
        assert_eq!(back.keep, ["放在桌面.docx"]);
        assert!(back.tiles[1].locked && !back.tiles[0].locked);
        assert_eq!((back.opacity, Config::parse("").opacity), (25, 100));
        // 旧格式（没有锁定列）照样能读
        assert!(!Config::parse("tile|文档|1|2|3|4|0|0").tiles[0].locked);
        assert_eq!(Config::parse("\u{feff}opacity|50").opacity, 50);
        assert_eq!(back.tiles.len(), 8);
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
    fn deleted_fallback_keeps_unknown_on_desktop() {
        let mut cfg = Config::parse("");
        cfg.rules.retain(|(_, t)| t != "其他");
        assert_eq!(classify(&cfg, "未知.xyz", false, false), None);
        assert_eq!(classify(&cfg, "周报.docx", false, false), Some("文档"));
    }
}
