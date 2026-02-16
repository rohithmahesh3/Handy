use ferrous_opencc::{config::BuiltinConfig, OpenCC};

pub fn convert_chinese_variant(text: &str, language: &str) -> String {
    let is_simplified = language == "zh-Hans";
    let is_traditional = language == "zh-Hant";

    if !is_simplified && !is_traditional {
        return text.to_string();
    }

    let config = if is_simplified {
        BuiltinConfig::Tw2sp
    } else {
        BuiltinConfig::S2twp
    };

    if let Ok(converter) = OpenCC::from_config(config) {
        converter.convert(text)
    } else {
        text.to_string()
    }
}
