//! Las tablas single-byte generadas coinciden con `data/indexes.json` del Encoding Standard, y
//! cada label de `data/encodings.json` resuelve a su encoding.

use bf_infra::encoding::Encoding;
use serde_json::Value;

fn data(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn single_byte_tables_match_indexes() {
    let indexes = data("indexes.json");
    let groups = data("encodings.json");
    let mut checked = 0;
    for group in groups.as_array().unwrap() {
        for e in group["encodings"].as_array().unwrap() {
            let name = e["name"].as_str().unwrap();
            for label in e["labels"].as_array().unwrap() {
                let enc = Encoding::for_label(label.as_str().unwrap()).unwrap();
                assert_eq!(enc.name(), name, "label {label}");
            }
            if group["heading"] != "Legacy single-byte encodings" {
                continue;
            }
            let index_name = if name == "ISO-8859-8-I" {
                "iso-8859-8".to_owned()
            } else {
                name.to_lowercase()
            };
            let index = indexes[&index_name].as_array().unwrap();
            let enc = Encoding::for_label(name).unwrap();
            for (i, cp) in index.iter().enumerate() {
                let byte = 0x80 + i as u8;
                let decoded = enc.decode_without_bom(&[byte]);
                match cp.as_u64() {
                    Some(cp) => {
                        let c = char::from_u32(cp as u32).unwrap();
                        assert_eq!(decoded, c.to_string(), "{name} 0x{byte:02X}");
                        assert_eq!(enc.encode_char(c), Some(vec![byte]), "{name} U+{cp:04X}");
                    }
                    None => assert_eq!(decoded, "\u{FFFD}", "{name} 0x{byte:02X}"),
                }
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 28);
}
