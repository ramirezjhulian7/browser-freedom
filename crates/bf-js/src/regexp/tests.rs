//! Tests del motor: los ejemplos de ECMA-262 §22.2.2, backtracking, cuantificadores,
//! lookbehind, case folding, modo `v`, modificadores y Annex B.

use super::Regex;

fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// El primer match buscando desde 0: el texto de cada captura.
fn exec_units(p: &str, f: &str, input: &[u16]) -> Option<Vec<Option<String>>> {
    let r = Regex::new(&units(p), f).unwrap_or_else(|e| panic!("/{p}/{f}: {e}"));
    let (_, caps) = r
        .search_from(input, 0, f.contains('y'), None)
        .expect("límite")?;
    Some(
        caps.iter()
            .map(|c| c.map(|(a, b)| String::from_utf16_lossy(&input[a..b])))
            .collect(),
    )
}

fn exec(p: &str, f: &str, s: &str) -> Option<Vec<Option<String>>> {
    exec_units(p, f, &units(s))
}

fn test(p: &str, f: &str, s: &str) -> bool {
    exec(p, f, s).is_some()
}

/// Atajo para los resultados esperados: `None` es una captura undefined.
fn caps(v: &[Option<&str>]) -> Option<Vec<Option<String>>> {
    Some(v.iter().map(|c| c.map(str::to_owned)).collect())
}

#[test]
fn spec_examples_alternatives() {
    // §22.2.2.3 Disjunction.
    assert_eq!(exec("a|ab", "", "abc"), caps(&[Some("a")]));
    assert_eq!(
        exec("((a)|(ab))((c)|(bc))", "", "abc"),
        caps(&[
            Some("abc"),
            Some("a"),
            Some("a"),
            None,
            Some("bc"),
            None,
            Some("bc")
        ])
    );
}

#[test]
fn spec_examples_quantifiers() {
    // §22.2.2.3.1 RepeatMatcher.
    assert_eq!(exec("a[a-z]{2,4}", "", "abcdefghi"), caps(&[Some("abcde")]));
    assert_eq!(exec("a[a-z]{2,4}?", "", "abcdefghi"), caps(&[Some("abc")]));
    assert_eq!(
        exec("(aa|aabaac|ba|b|c)*", "", "aabaac"),
        caps(&[Some("aaba"), Some("ba")])
    );
    assert_eq!(
        exec("(z)((a+)?(b+)?(c))*", "", "zaacbbbcac"),
        caps(&[
            Some("zaacbbbcac"),
            Some("z"),
            Some("ac"),
            Some("a"),
            None,
            Some("c")
        ])
    );
    assert_eq!(exec("(a*)*", "", "b"), caps(&[Some(""), None]));
    assert_eq!(
        exec("(a*)b\\1+", "", "baaaac"),
        caps(&[Some("b"), Some("")])
    );
}

#[test]
fn spec_examples_lookahead() {
    assert_eq!(
        exec("(?=(a+))", "", "baaabac"),
        caps(&[Some(""), Some("aaa")])
    );
    assert_eq!(
        exec("(?=(a+))a*b\\1", "", "baaabac"),
        caps(&[Some("aba"), Some("a")])
    );
    assert_eq!(
        exec("(.*?)a(?!(a+)b\\2c)\\2(.*)", "", "baaabaac"),
        caps(&[Some("baaabaac"), Some("ba"), None, Some("abaac")])
    );
}

#[test]
fn empty_iteration_check() {
    assert_eq!(exec("(a|)*b", "", "aab"), caps(&[Some("aab"), Some("a")]));
    // Un cuerpo que puede ser vacío no entra en un bucle infinito.
    assert_eq!(exec("(?:a?)*$", "", "aa"), caps(&[Some("aa")]));
    assert_eq!(exec("(a?)+?b", "", "b"), caps(&[Some("b"), Some("")]));
}

#[test]
fn captures_reset_each_iteration() {
    assert_eq!(exec("(?:(a)|b)+", "", "ab"), caps(&[Some("ab"), None]));
    assert_eq!(
        exec("(?:(a)|(b))+", "", "ab"),
        caps(&[Some("ab"), None, Some("b")])
    );
}

#[test]
fn lazy_and_greedy_simple_loops() {
    assert_eq!(exec("a+?b", "", "aaab"), caps(&[Some("aaab")]));
    assert_eq!(exec("a{2,}?", "", "aaaa"), caps(&[Some("aa")]));
    assert_eq!(exec("a{2,3}", "", "aaaa"), caps(&[Some("aaa")]));
    assert_eq!(exec("x*y", "", "xxxz xy"), caps(&[Some("xy")]));
    assert_eq!(exec("a{0}b", "", "ab"), caps(&[Some("b")]));
    assert!(!test("^a{3}$", "", "aa"));
}

#[test]
fn lookbehind() {
    assert_eq!(
        exec("(?<=\\$)\\d+(\\.\\d*)?", "", "cost $10.53"),
        caps(&[Some("10.53"), Some(".53")])
    );
    assert_eq!(exec("(?<!\\$)\\d+", "", "$10 20"), caps(&[Some("0")]));
    // La captura de la derecha se evalúa primero (dirección backward).
    assert_eq!(
        exec("(?<=(\\d+)(\\d+))$", "", "1053"),
        caps(&[Some(""), Some("1"), Some("053")])
    );
    assert_eq!(exec("(?<=(o)d\\1)r", "", "hodor"), None);
    assert_eq!(
        exec("(?<=\\1d(o))r", "", "hodor"),
        caps(&[Some("r"), Some("o")])
    );
    assert_eq!(
        exec("(?<=(a+))b", "", "aaab"),
        caps(&[Some("b"), Some("aaa")])
    );
    assert_eq!(exec("(?<=a(?=b)b)c", "", "abc"), caps(&[Some("c")]));
}

#[test]
fn lookbehind_unicode() {
    assert_eq!(exec("(?<=😀)a", "u", "😀a"), caps(&[Some("a")]));
    assert_eq!(exec("(?<=^.)a", "u", "😀a"), caps(&[Some("a")]));
    assert_eq!(exec("(?<=^.)a", "", "😀a"), None);
}

#[test]
fn backreferences() {
    assert_eq!(exec("\\1(a)", "", "a"), caps(&[Some("a"), Some("a")]));
    assert_eq!(exec("(a\\1)", "", "aa"), caps(&[Some("a"), Some("a")]));
    assert_eq!(exec("(a)\\1", "i", "aA"), caps(&[Some("aA"), Some("a")]));
    assert_eq!(
        exec("(?<y>\\d{4})-(?<m>\\d{2})", "", "on 2026-10"),
        caps(&[Some("2026-10"), Some("2026"), Some("10")])
    );
    assert_eq!(
        exec("(?:(?<a>x)|(?<a>y))\\k<a>", "", "yy"),
        caps(&[Some("yy"), None, Some("y")])
    );
    assert_eq!(
        exec("\\k<a>(?<a>b)", "", "b"),
        caps(&[Some("b"), Some("b")])
    );
}

#[test]
fn case_folding_non_unicode() {
    assert!(test("[a-z]", "i", "K"));
    assert!(!test("\\u212a", "i", "k"));
    assert!(!test("\\u017f", "i", "s"));
    assert!(!test("\\w", "i", "\u{17f}"));
    assert!(test("\\u00e5", "i", "\u{c5}"));
    // ß no tiene una mayúscula de un carácter.
    assert!(!test("\\u00df", "i", "\u{1e9e}"));
}

#[test]
fn case_folding_unicode() {
    assert!(test("\\u212a", "iu", "k"));
    assert!(test("\\u017f", "iu", "S"));
    assert!(test("\\w", "iu", "\u{17f}"));
    assert!(test("\\w", "iu", "\u{212a}"));
    assert!(!test("\\W", "iu", "S"));
    assert!(test("\\b", "iu", "\u{17f}"));
    assert!(test("[\\u{10400}]", "iu", "\u{10428}"));
}

#[test]
fn unicode_mode_input() {
    assert!(test("^.$", "u", "😀"));
    assert!(!test("^.$", "", "😀"));
    assert!(test("^..$", "", "😀"));
    let pair = [0xD834u16, 0xDF06];
    assert!(exec_units("\\udf06", "u", &pair).is_none());
    assert!(exec_units("\\udf06", "", &pair).is_some());
    assert!(exec_units("^[\\ud834\\udf06]$", "u", &pair).is_some());
    // Un surrogate suelto es un carácter.
    assert!(exec_units("^.$", "u", &[0xD834]).is_some());
}

#[test]
fn unicode_sets_mode() {
    assert!(!test("[\\p{ASCII}--\\p{Lowercase}]", "v", "a"));
    assert!(test("[\\p{ASCII}--\\p{Lowercase}]", "v", "A"));
    assert_eq!(exec("[\\q{abc|a}]", "v", "abc"), caps(&[Some("abc")]));
    assert_eq!(exec("^[\\q{abc|a}]c$", "v", "ac"), caps(&[Some("ac")]));
    assert_eq!(exec("[[a-z]&&[aeiou]]+", "v", "xaeb"), caps(&[Some("ae")]));
    assert_eq!(
        exec("\\p{RGI_Emoji_Flag_Sequence}", "v", "x🇪🇸"),
        caps(&[Some("🇪🇸")])
    );
    assert!(test("^[\\q{AB}]$", "vi", "ab"));
    assert!(!test("[^a]", "vi", "A"));
    assert!(test("[^a]", "vi", "b"));
    assert_eq!(exec("(?<=[\\q{ab}])c", "v", "abc"), caps(&[Some("c")]));
    assert_eq!(exec("^[\\q{}a]b", "v", "b"), caps(&[Some("b")]));
    assert!(test("^\\p{RGI_Emoji}$", "v", "👨‍👩‍👧"));
}

#[test]
fn modifiers() {
    assert!(test("a(?i:b)c", "", "aBc"));
    assert!(!test("a(?i:b)c", "", "ABc"));
    assert!(test("(?-i:a)b", "i", "aB"));
    assert!(!test("(?-i:a)b", "i", "AB"));
    assert!(test("(?m:^b)", "", "a\nb"));
    assert!(test("(?s:.)", "", "\n"));
    assert!(!test("(?-s:.)", "s", "\n"));
}

#[test]
fn assertions() {
    assert!(test("^b", "m", "a\nb"));
    assert!(!test("^b", "", "a\nb"));
    assert!(test("a$", "m", "a\nb"));
    assert!(!test("a$", "", "a\nb"));
    assert!(test("a.b", "s", "a\nb"));
    assert!(!test("a.b", "", "a\nb"));
    assert_eq!(exec("\\bfoo\\b", "", "a foo."), caps(&[Some("foo")]));
    assert_eq!(exec("\\Boo", "", "foo"), caps(&[Some("oo")]));
}

#[test]
fn annex_b_syntax() {
    assert!(test("^\\101$", "", "A"));
    assert!(test("^[\\b]$", "", "\u{8}"));
    assert!(test("^\\cJ$", "", "\n"));
    assert!(test("^\\c$", "", "\\c"));
    assert!(test("^[\\c_]$", "", "\u{1f}"));
    assert!(test("^[\\d-a]+$", "", "1-a"));
    assert!(test("^a{$", "", "a{"));
    assert!(test("^\\8$", "", "8"));
    // `\2` con un solo grupo es un escape octal.
    assert!(test("^(a)\\2$", "", "a\u{2}"));
    // Lookahead cuantificado.
    assert!(test("^(?=a)*a", "", "a"));
}

#[test]
fn sticky_and_classes() {
    let r = Regex::new(&units("b"), "y").unwrap();
    let s = units("ab");
    assert!(r.search_from(&s, 0, true, None).unwrap().is_none());
    assert!(r.search_from(&s, 1, true, None).unwrap().is_some());
    assert!(test("^[^\\d\\s]+$", "", "abc"));
    assert!(test("^\\S\\s\\D\\d\\W\\w$", "", "a x1!_"));
    assert!(test("^\\p{Lu}\\p{Script=Greek}\\P{L}$", "u", "Aα1"));
    assert!(test("^\\p{scx=Hira}$", "u", "\u{30fc}"));
}

#[test]
fn catastrophic_patterns_are_bounded() {
    // Exponencial, pero con el input corto termina; el límite de pasos protege el resto.
    assert_eq!(exec("(a+)+b", "", "aaaaaaaaaaaaaaaa"), None);
    let r = Regex::new(&units("(a*)*b"), "").unwrap();
    let s = units(&"a".repeat(200));
    // Puede tardar mucho: basta con que no haya pánico (o devuelva Limit).
    let _ = r.match_at(&s[..18], 0, None);
}
