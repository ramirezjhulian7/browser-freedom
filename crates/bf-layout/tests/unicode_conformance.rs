//! Conformidad de UAX #9 (`BidiTest.txt`, `BidiCharacterTest.txt`) y UAX #14
//! (`LineBreakTest.txt`) con los archivos de la UCD 17.0.0 en `tests/data/`.

use bf_layout::bidi::{BidiClass, apply_l1, reorder, resolve_paragraph};
use bf_layout::linebreak::{Break, line_breaks};

fn data(name: &str) -> String {
    let path = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn parse_levels(s: &str) -> Vec<Option<u8>> {
    s.split_whitespace()
        .map(|t| if t == "x" { None } else { Some(t.parse().unwrap()) })
        .collect()
}

/// Niveles después de L1 (toda la entrada como una línea) y orden visual de L2 sin los
/// caracteres que quita X9.
fn run(classes: &[BidiClass], chars: Option<&[u32]>, base: Option<u8>) -> (u8, Vec<Option<u8>>, Vec<usize>) {
    let r = resolve_paragraph(classes, chars, base);
    let mut levels = r.levels.clone();
    apply_l1(classes, &mut levels, r.para_level);
    let kept: Vec<usize> = (0..levels.len()).filter(|&i| levels[i].is_some()).collect();
    let lv: Vec<u8> = kept.iter().map(|&i| levels[i].unwrap()).collect();
    let order = reorder(&lv).into_iter().map(|k| kept[k]).collect();
    (r.para_level, levels, order)
}

#[test]
fn bidi_test() {
    let text = data("BidiTest.txt");
    let mut levels: Vec<Option<u8>> = Vec::new();
    let mut order: Vec<usize> = Vec::new();
    let (mut pass, mut fail) = (0usize, 0usize);
    let mut first_failures = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap().trim();
        if line.is_empty() {
            continue;
        }
        if let Some(l) = line.strip_prefix("@Levels:") {
            levels = parse_levels(l);
            continue;
        }
        if let Some(o) = line.strip_prefix("@Reorder:") {
            order = o.split_whitespace().map(|t| t.parse().unwrap()).collect();
            continue;
        }
        if line.starts_with('@') {
            continue;
        }
        let (input, bits) = line.split_once(';').unwrap();
        let classes: Vec<BidiClass> = input
            .split_whitespace()
            .map(|t| BidiClass::from_name(t).unwrap())
            .collect();
        let bits = u8::from_str_radix(bits.trim(), 16).unwrap();
        for (bit, base) in [(1, None), (2, Some(0)), (4, Some(1))] {
            if bits & bit == 0 {
                continue;
            }
            let (_, got_levels, got_order) = run(&classes, None, base);
            if got_levels == levels && got_order == order {
                pass += 1;
            } else {
                fail += 1;
                if first_failures.len() < 10 {
                    first_failures.push(format!(
                        "{input} base={base:?}: niveles {got_levels:?} (esperado {levels:?}), orden {got_order:?} (esperado {order:?})"
                    ));
                }
            }
        }
    }
    println!("BidiTest.txt: {pass} pasan, {fail} fallan");
    assert!(fail == 0, "{fail} fallos:\n{}", first_failures.join("\n"));
}

#[test]
fn bidi_character_test() {
    let text = data("BidiCharacterTest.txt");
    let (mut pass, mut fail) = (0usize, 0usize);
    let mut first_failures = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split(';').collect();
        let cps: Vec<u32> = f[0].split_whitespace().map(|t| u32::from_str_radix(t, 16).unwrap()).collect();
        let classes: Vec<BidiClass> = cps
            .iter()
            .map(|&c| bf_layout::bidi::bidi_class(char::from_u32(c).unwrap()))
            .collect();
        let base = match f[1].trim() {
            "0" => Some(0),
            "1" => Some(1),
            _ => None,
        };
        let para: u8 = f[2].trim().parse().unwrap();
        let levels = parse_levels(f[3]);
        let order: Vec<usize> = f[4].split_whitespace().map(|t| t.parse().unwrap()).collect();
        let (got_para, got_levels, got_order) = run(&classes, Some(&cps), base);
        if got_para == para && got_levels == levels && got_order == order {
            pass += 1;
        } else {
            fail += 1;
            if first_failures.len() < 10 {
                first_failures.push(format!(
                    "{}: niveles {got_levels:?} (esperado {levels:?}), orden {got_order:?} (esperado {order:?})",
                    f[0]
                ));
            }
        }
    }
    println!("BidiCharacterTest.txt: {pass} pasan, {fail} fallan");
    assert!(fail == 0, "{fail} fallos:\n{}", first_failures.join("\n"));
}

#[test]
fn line_break_test() {
    let text = data("LineBreakTest.txt");
    let (mut pass, mut fail) = (0usize, 0usize);
    let mut first_failures = Vec::new();
    for line in text.lines() {
        let body = line.split('#').next().unwrap().trim();
        if body.is_empty() {
            continue;
        }
        let mut chars = Vec::new();
        let mut expected = Vec::new();
        for tok in body.split_whitespace() {
            match tok {
                "÷" => expected.push(true),
                "×" => expected.push(false),
                hex => chars.push(char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap()),
            }
        }
        let got: Vec<bool> = line_breaks(&chars).iter().map(|b| *b != Break::Prohibited).collect();
        // La posición 0 (sot) del test es siempre ×.
        if got == expected {
            pass += 1;
        } else {
            fail += 1;
            if first_failures.len() < 15 {
                first_failures.push(format!("{body}\n  obtenido: {got:?}\n  esperado: {expected:?}"));
            }
        }
    }
    println!("LineBreakTest.txt: {pass} pasan, {fail} fallan");
    assert!(fail == 0, "{fail} fallos:\n{}", first_failures.join("\n"));
}
