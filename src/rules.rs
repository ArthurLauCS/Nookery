//! 归类规则与配置文件。纯逻辑，不碰 Win32。

/// 规则键：目录
pub const FOLDER_KEY: &str = "/";
/// 规则键：没有专门规则的其他文件
pub const FALLBACK_KEY: &str = "*";

/// 快捷方式留在桌面，不归类
const SHORTCUT_EXT: [&str; 4] = ["lnk", "url", "appref-ms", "website"];
/// 还在写入中的临时文件
const TEMP_EXT: [&str; 5] = ["crdownload", "part", "partial", "download", "tmp"];

const DEFAULT_TILES: [(&str, &str); 8] = [
    ("文件夹", "/"),
    ("文档", "doc docx pdf txt md rtf odt xls xlsx csv ppt pptx wps et dps"),
    ("图片", "png jpg jpeg gif webp bmp svg heic ico psd"),
    ("视频", "mp4 mkv mov avi wmv flv webm"),
    ("音频", "mp3 wav flac aac m4a ogg"),
    ("压缩包", "zip rar 7z tar gz"),
    ("程序", "exe msi"),
    ("其他", "*"),
];

pub struct TileCfg {
    pub name: String,
    /// 物理像素 x, y, w, h；w == 0 表示还没摆放过
    pub rect: [i32; 4],
    pub rolled: bool,
    /// 用户新建的磁贴，空了也显示
    pub keep: bool,
}

pub struct Config {
    pub tiles: Vec<TileCfg>,
    /// (扩展名或 FOLDER_KEY / FALLBACK_KEY, 磁贴名)
    pub rules: Vec<(String, String)>,
    pub paused: bool,
    /// 用户从磁贴拖回桌面的项，留在桌面不再收回
    pub keep: Vec<String>,
}

impl Config {
    /// 空文本得到默认配置
    pub fn parse(text: &str) -> Config {
        let mut cfg = Config { tiles: Vec::new(), rules: Vec::new(), paused: false, keep: Vec::new() };
        for line in text.lines() {
            let f: Vec<&str> = line.split('|').collect();
            match f.as_slice() {
                ["paused", v] => cfg.paused = *v == "1",
                ["keep", name] => cfg.keep.push(name.to_string()),
                ["rule", key, tile] => cfg.rules.push((key.to_string(), tile.to_string())),
                ["tile", name, x, y, w, h, rolled, keep] => cfg.tiles.push(TileCfg {
                    name: name.to_string(),
                    rect: [x, y, w, h].map(|v| v.parse().unwrap_or(0)),
                    rolled: *rolled == "1",
                    keep: *keep == "1",
                }),
                _ => {}
            }
        }
        if cfg.tiles.is_empty() && cfg.rules.is_empty() {
            for (name, keys) in DEFAULT_TILES {
                cfg.tiles.push(TileCfg { name: name.into(), rect: [0; 4], rolled: false, keep: false });
                cfg.rules.extend(keys.split(' ').map(|k| (k.to_string(), name.to_string())));
            }
        }
        cfg
    }

    pub fn to_text(&self) -> String {
        let mut s = format!("paused|{}\n", self.paused as u8);
        for t in &self.tiles {
            let [x, y, w, h] = t.rect;
            s += &format!("tile|{}|{x}|{y}|{w}|{h}|{}|{}\n", t.name, t.rolled as u8, t.keep as u8);
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
        cfg.keep.push("放在桌面.docx".into());
        let back = Config::parse(&cfg.to_text());
        assert_eq!(classify(&back, "说明.pdf", false, false), Some("图片"));
        assert_eq!(back.tiles[1].rect, [10, 20, 300, 200]);
        assert!(back.tiles[1].rolled && back.paused);
        assert_eq!(back.keep, ["放在桌面.docx"]);
        assert_eq!(back.tiles.len(), 8);
    }

    #[test]
    fn deleted_fallback_keeps_unknown_on_desktop() {
        let mut cfg = Config::parse("");
        cfg.rules.retain(|(_, t)| t != "其他");
        assert_eq!(classify(&cfg, "未知.xyz", false, false), None);
        assert_eq!(classify(&cfg, "周报.docx", false, false), Some("文档"));
    }
}
