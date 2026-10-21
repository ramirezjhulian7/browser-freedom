"""Genera crates/bf-js/src/builtins/intl/cldr.rs: los datos de CLDR que usa Intl (ECMA-402).

Desde el subconjunto de cldr-json 48.0.0 en crates/bf-js/data/cldr-48.0.0/:

- metadatos (supplemental, bcp47): alias de idioma, script, región, variante y subdivisión
  (UTS #35 §3.2.1, canonicalización), las regiones probables de cada idioma (likelySubtags,
  para los alias de región con varios reemplazos), los alias de los valores de las claves
  Unicode (`-u-ca-gregorian` → `gregory`…), los time zones IANA con su nombre canónico, la
  tabla de zonas de Windows, los dígitos de cada moneda, los sistemas de numeración
  numéricos, el ciclo horario preferido por región y las reglas de plurales;
- datos de locale solo para `en` y `es`: símbolos y patrones numéricos (decimal, porcentaje,
  moneda, contable, científico y compacto), monedas, unidades y el calendario gregoriano.

Uso: python -I tools/gen_intl_data.py
"""

import json
import os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(ROOT, "crates", "bf-js", "data", "cldr-48.0.0")
OUT = os.path.join(ROOT, "crates", "bf-js", "src", "builtins", "intl", "cldr.rs")
LOCALES = ["en", "es"]

# ECMA-402 §6.6.2, tabla "Simple units sanctioned for use in ECMAScript".
SANCTIONED_UNITS = [
    "acre", "bit", "byte", "celsius", "centimeter", "day", "degree", "fahrenheit",
    "fluid-ounce", "foot", "gallon", "gigabit", "gigabyte", "gram", "hectare", "hour", "inch",
    "kilobit", "kilobyte", "kilogram", "kilometer", "liter", "megabit", "megabyte", "meter",
    "microsecond", "mile", "mile-scandinavian", "milliliter", "millimeter", "millisecond",
    "minute", "month", "nanosecond", "ounce", "percent", "petabyte", "pound", "second", "stone",
    "terabit", "terabyte", "week", "yard", "year",
]


def load(*parts):
    with open(os.path.join(DATA, *parts), encoding="utf-8") as f:
        return json.load(f)


def rs(s):
    """Un literal de string de Rust."""
    out = []
    for c in s:
        if c == "\\":
            out.append("\\\\")
        elif c == '"':
            out.append('\\"')
        elif ord(c) < 0x20 or ord(c) > 0x7E:
            out.append("\\u{%X}" % ord(c))
        else:
            out.append(c)
    return '"' + "".join(out) + '"'


def pairs(name, items, doc):
    lines = [f"/// {doc}", f"pub static {name}: &[(&str, &str)] = &["]
    for k, v in items:
        lines.append(f"    ({rs(k)}, {rs(v)}),")
    lines.append("];")
    return "\n".join(lines)


def str_list(items):
    return "&[" + ", ".join(rs(s) for s in items) + "]"


def main():
    out = [
        "//! Datos de CLDR 48.0.0 para Intl. Generado por `tools/gen_intl_data.py`: no editar.",
        "",
        "#![allow(clippy::type_complexity)]",
        "",
        'pub const CLDR_VERSION: &str = "48.0.0";',
        "",
    ]

    # --- Alias (UTS #35 §3.2.1, "Canonical Unicode Locale Identifiers") ---
    alias = load("supplemental", "aliases.json")["supplemental"]["metadata"]["alias"]
    for key, name, doc in [
        ("languageAlias", "LANGUAGE_ALIASES", "languageAlias: tag (idioma o idioma-variante) → reemplazo."),
        ("scriptAlias", "SCRIPT_ALIASES", "scriptAlias."),
        ("territoryAlias", "REGION_ALIASES", "territoryAlias: los reemplazos múltiples van separados por espacios."),
        ("variantAlias", "VARIANT_ALIASES", "variantAlias."),
        ("subdivisionAlias", "SUBDIVISION_ALIASES", "subdivisionAlias (valores de `rg` y `sd`)."),
    ]:
        items = sorted((k.lower().replace("_", "-") if key != "scriptAlias" else k, v["_replacement"]) for k, v in alias[key].items())
        out.append(pairs(name, items, doc))
        out.append("")

    likely = load("supplemental", "likelySubtags.json")["supplemental"]["likelySubtags"]
    regions = []
    for k, v in likely.items():
        k = k.replace("_", "-")
        if "-" in k and not (k.startswith("und-") and len(k) == 8):
            continue
        k = k.lower()
        parts = v.replace("_", "-").split("-")
        if len(parts) >= 3:
            regions.append((k, parts[-1]))
    out.append(pairs("LIKELY_REGION", sorted(regions), "La región probable de cada idioma (likelySubtags)."))
    out.append("")

    # --- Claves Unicode (BCP 47) ---
    value_aliases = []
    valid_values = {}
    tz_rows = []
    for fname in sorted(os.listdir(os.path.join(DATA, "bcp47"))):
        kw = load("bcp47", fname)["keyword"]
        for ext, keys in kw.items():
            for key, values in keys.items():
                if not isinstance(values, dict):
                    continue
                for value, info in values.items():
                    if value.startswith("_") or not isinstance(info, dict):
                        continue
                    full = key if ext == "u" else f"{ext}:{key}"
                    valid_values.setdefault(full, []).append(value)
                    preferred = info.get("_preferred")
                    if key == "tz" and ext == "u":
                        names = info.get("_alias", "").split()
                        if names:
                            primary = info.get("_iana", names[0])
                            if value in ("utc", "gmt"):
                                primary = "UTC"
                            tz_rows.append((value, [primary] + [n for n in names if n != primary], preferred, info.get("_deprecated") == "true"))
                        if preferred:
                            value_aliases.append((full, value, preferred))
                        continue
                    if preferred:
                        value_aliases.append((full, value, preferred))
                        continue
                    if info.get("_deprecated") == "true":
                        continue
                    for a in info.get("_alias", "").split():
                        if a.lower() != value:
                            value_aliases.append((full, a.lower(), value))
    value_aliases = sorted(set(value_aliases))
    out.append("/// Alias de los valores de las claves de extensión: (clave, alias, canónico).")
    out.append("pub static KEYWORD_VALUE_ALIASES: &[(&str, &str, &str)] = &[")
    for k, a, c in value_aliases:
        out.append(f"    ({rs(k)}, {rs(a)}, {rs(c)}),")
    out.append("];")
    out.append("")
    out.append("/// Valores válidos de cada clave de extensión (`ca`, `co`, `nu`…).")
    out.append("pub static KEYWORD_VALUES: &[(&str, &[&str])] = &[")
    for k in sorted(valid_values):
        if k == "tz":
            continue
        out.append(f"    ({rs(k)}, {str_list(sorted(set(valid_values[k])))}),")
    out.append("];")
    out.append("")

    # Time zones: (nombre IANA, nombre canónico IANA, id BCP 47).
    zone_alias = {}
    for area, zones in alias["zoneAlias"].items():
        def walk(prefix, node):
            for k, v in node.items():
                if isinstance(v, dict) and "_replacement" in v:
                    zone_alias[prefix + k] = v["_replacement"]
                elif isinstance(v, dict):
                    walk(prefix + k + "/", v)
        walk(area + "/", zones)
    by_id = {row[0]: row for row in tz_rows}
    tz_names = {}
    for bid, names, preferred, deprecated in tz_rows:
        target = by_id.get(preferred, (bid, names))[1][0] if preferred else names[0]
        for n in names:
            tz_names[n] = target
    for n, r in zone_alias.items():
        tz_names.setdefault(n, tz_names.get(r, r))
    tz_names["UTC"] = "UTC"
    rows = sorted(tz_names.items(), key=lambda kv: kv[0].lower())
    out.append(pairs("TIME_ZONES", rows, "Identificador IANA → identificador primario (ordenados sin distinguir mayúsculas)."))
    out.append("")

    win = load("supplemental", "windowsZones.json")["supplemental"]["windowsZones"]["mapTimezones"]
    wz = sorted({(m["mapZone"]["_other"], m["mapZone"]["_type"].split()[0]) for m in win if m["mapZone"]["_territory"] == "001"})
    out.append(pairs("WINDOWS_ZONES", wz, "Zona de Windows (TimeZoneKeyName) → zona IANA (territorio 001)."))
    out.append("")

    cur = load("supplemental", "currencyData.json")["supplemental"]["currencyData"]["fractions"]
    out.append("/// Dígitos de fracción de cada moneda (CurrencyDigits, ECMA-402 §16.5.1); el resto, 2.")
    out.append("pub static CURRENCY_DIGITS: &[(&str, u8)] = &[")
    for code in sorted(cur):
        if code == "DEFAULT":
            continue
        out.append(f"    ({rs(code)}, {int(cur[code]['_digits'])}),")
    out.append("];")
    out.append("")

    ns = load("supplemental", "numberingSystems.json")["supplemental"]["numberingSystems"]
    out.append(pairs("NUMBERING_SYSTEMS", sorted((k, v["_digits"]) for k, v in ns.items() if v.get("_type") == "numeric"),
                     "Sistemas de numeración numéricos y sus diez dígitos (ECMA-402 Table 14)."))
    out.append("")

    td = load("supplemental", "timeData.json")["supplemental"]["timeData"]
    out.append(pairs("HOUR_CYCLES", sorted((k, v["_preferred"]) for k, v in td.items()),
                     "El símbolo de hora preferido por región (timeData)."))
    out.append("")

    plurals = []
    for fname, kind in [("plurals.json", "cardinal"), ("ordinals.json", "ordinal")]:
        data = load("supplemental", fname)["supplemental"][f"plurals-type-{kind}"]
        for loc in LOCALES:
            for k, rule in data[loc].items():
                cat = k.replace("pluralRule-count-", "")
                cond = rule.split("@")[0].strip()
                plurals.append((loc, kind, cat, cond))
    out.append("/// Reglas de plurales (UTS #35, Part 3 §5): (locale, tipo, categoría, condición).")
    out.append("pub static PLURAL_RULES: &[(&str, &str, &str, &str)] = &[")
    for row in plurals:
        out.append("    (" + ", ".join(rs(x) for x in row) + "),")
    out.append("];")
    out.append("")

    # --- Datos de locale ---
    out.append(LOCALE_TYPES)
    for loc in LOCALES:
        out.append(locale_data(loc))
    out.append("pub static LOCALE_DATA: &[(&str, &LocaleData)] = &[")
    for loc in LOCALES:
        out.append(f"    ({rs(loc)}, &{loc.upper()}),")
    out.append("];")

    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(out) + "\n")
    print(f"escrito {OUT}")


LOCALE_TYPES = '''
/// Patrones compactos: (magnitud, categoría de plural, patrón).
pub type Compact = &'static [(u8, &'static str, &'static str)];

/// Los datos numéricos de un locale (sistema `latn`).
pub struct NumberData {
    pub decimal: &'static str,
    pub group: &'static str,
    pub percent: &'static str,
    pub plus: &'static str,
    pub minus: &'static str,
    pub exponential: &'static str,
    pub infinity: &'static str,
    pub nan: &'static str,
    pub approximately: &'static str,
    /// miscPatterns: `approximately` (`~{0}`) y `range` (`{0}–{1}`).
    pub approximately_pattern: &'static str,
    pub range_pattern: &'static str,
    pub min_grouping: u8,
    pub decimal_pattern: &'static str,
    pub percent_pattern: &'static str,
    pub scientific_pattern: &'static str,
    pub currency_pattern: &'static str,
    pub currency_alpha_pattern: &'static str,
    pub accounting_pattern: &'static str,
    pub accounting_alpha_pattern: &'static str,
    pub currency_unit_one: &'static str,
    pub currency_unit_other: &'static str,
    pub compact_short: Compact,
    pub compact_long: Compact,
    pub compact_currency: Compact,
    /// (código, símbolo, símbolo estrecho, nombre singular, nombre plural, nombre).
    pub currencies: &'static [(&'static str, &'static str, &'static str, &'static str, &'static str, &'static str)],
    /// (unidad, ancho, singular, plural, patrón "por").
    pub units: &'static [(&'static str, &'static str, &'static str, &'static str, &'static str)],
    /// El patrón compuesto `{0}/{1}` por ancho (long, short, narrow).
    pub per_patterns: [&'static str; 3],
}

/// Los datos del calendario gregoriano de un locale.
pub struct DateData {
    /// Meses: format wide, abbreviated, narrow; stand-alone wide, abbreviated, narrow.
    pub months: [[&'static str; 12]; 6],
    /// Días (domingo primero): format wide, abbreviated, short, narrow; stand-alone igual.
    pub days: [[&'static str; 7]; 8],
    /// AM/PM: abbreviated, wide, narrow.
    pub day_periods: [[&'static str; 2]; 3],
    /// Períodos flexibles (`B`): (nombre, abreviado, ancho, estrecho).
    pub flexible_periods: &'static [(&'static str, &'static str, &'static str, &'static str)],
    /// Eras (BC, AD): abbreviated, wide, narrow.
    pub eras: [[&'static str; 2]; 3],
    pub date_formats: [&'static str; 4],
    pub time_formats: [&'static str; 4],
    pub date_time_formats: [&'static str; 4],
    /// availableFormats: (skeleton, patrón).
    pub available: &'static [(&'static str, &'static str)],
    /// intervalFormats: (skeleton, campo mayor que cambia, patrón).
    pub intervals: &'static [(&'static str, &'static str, &'static str)],
    pub interval_fallback: &'static str,
}

pub struct LocaleData {
    pub numbers: NumberData,
    pub dates: DateData,
}
'''


def compact(fmt):
    rows = []
    for k, v in fmt.items():
        if "-count-" not in k or "alt" in k:
            continue
        mag, count = k.split("-count-")
        rows.append((len(mag) - 1, count, v))
    rows.sort(key=lambda r: (r[0], r[1]))
    return "&[" + ", ".join(f"({m}, {rs(c)}, {rs(p)})" for m, c, p in rows) + "]"


def locale_data(loc):
    n = load(loc, "numbers.json")["main"][loc]["numbers"]
    sym = n["symbols-numberSystem-latn"]
    dec = n["decimalFormats-numberSystem-latn"]
    curf = n["currencyFormats-numberSystem-latn"]
    curs = load(loc, "currencies.json")["main"][loc]["numbers"]["currencies"]
    units = load(loc, "units.json")["main"][loc]["units"]
    g = load(loc, "ca-gregorian.json")["main"][loc]["dates"]["calendars"]["gregorian"]

    currencies = []
    for code in sorted(curs):
        c = curs[code]
        currencies.append(
            f"({rs(code)}, {rs(c.get('symbol', code))}, {rs(c.get('symbol-alt-narrow', c.get('symbol', code)))}, "
            f"{rs(c.get('displayName-count-one', c.get('displayName', code)))}, "
            f"{rs(c.get('displayName-count-other', c.get('displayName', code)))}, {rs(c.get('displayName', code))})"
        )
    unit_rows = []
    for width in ["long", "short", "narrow"]:
        table = units[width]
        for u in SANCTIONED_UNITS:
            key = next((k for k in table if k.split("-", 1)[-1] == u and not k.startswith("10p")), None)
            if key is None:
                continue
            e = table[key]
            unit_rows.append(
                f"({rs(u)}, {rs(width)}, {rs(e.get('unitPattern-count-one', e.get('unitPattern-count-other', '{0}')))}, "
                f"{rs(e.get('unitPattern-count-other', '{0}'))}, {rs(e.get('perUnitPattern', ''))})"
            )
    per = [units[w]["per"]["compoundUnitPattern"] for w in ["long", "short", "narrow"]]

    def month_list(ctx, width):
        m = g["months"][ctx][width]
        return "[" + ", ".join(rs(m[str(i)]) for i in range(1, 13)) + "]"

    def day_list(ctx, width):
        d = g["days"][ctx][width]
        return "[" + ", ".join(rs(d[k]) for k in ["sun", "mon", "tue", "wed", "thu", "fri", "sat"]) + "]"

    dp = g["dayPeriods"]["format"]
    flex = []
    for name in ["midnight", "noon", "morning1", "morning2", "afternoon1", "afternoon2", "evening1", "evening2", "night1", "night2"]:
        if name in dp["abbreviated"]:
            flex.append(f"({rs(name)}, {rs(dp['abbreviated'][name])}, {rs(dp['wide'].get(name, dp['abbreviated'][name]))}, {rs(dp['narrow'].get(name, dp['abbreviated'][name]))})")
    eras = g["eras"]
    df = g["dateFormats"]
    tf = g["timeFormats"]
    dtf = g["dateTimeFormats"]
    avail = sorted((k, v) for k, v in dtf["availableFormats"].items() if "-alt-" not in k and "-count-" not in k)
    intervals = []
    for sk, fields in dtf["intervalFormats"].items():
        if sk == "intervalFormatFallback" or not isinstance(fields, dict):
            continue
        for field, pat in fields.items():
            if "-alt-" in field:
                continue
            intervals.append((sk, field, pat))
    intervals.sort()

    def fmt(x):
        return x if isinstance(x, str) else x["_value"]

    lines = [
        f"static {loc.upper()}: LocaleData = LocaleData {{",
        "    numbers: NumberData {",
        f"        decimal: {rs(sym['decimal'])},",
        f"        group: {rs(sym['group'])},",
        f"        percent: {rs(sym['percentSign'])},",
        f"        plus: {rs(sym['plusSign'])},",
        f"        minus: {rs(sym['minusSign'])},",
        f"        exponential: {rs(sym['exponential'])},",
        f"        infinity: {rs(sym['infinity'])},",
        f"        nan: {rs(sym['nan'])},",
        f"        approximately: {rs(sym['approximatelySign'])},",
        f"        approximately_pattern: {rs(n['miscPatterns-numberSystem-latn']['approximately'])},",
        f"        range_pattern: {rs(n['miscPatterns-numberSystem-latn']['range'])},",
        f"        min_grouping: {int(n['minimumGroupingDigits'])},",
        f"        decimal_pattern: {rs(dec['standard'])},",
        f"        percent_pattern: {rs(n['percentFormats-numberSystem-latn']['standard'])},",
        f"        scientific_pattern: {rs(n['scientificFormats-numberSystem-latn']['standard'])},",
        f"        currency_pattern: {rs(curf['standard'])},",
        f"        currency_alpha_pattern: {rs(curf.get('standard-alphaNextToNumber', curf['standard']))},",
        f"        accounting_pattern: {rs(curf['accounting'])},",
        f"        accounting_alpha_pattern: {rs(curf.get('accounting-alphaNextToNumber', curf['accounting']))},",
        f"        currency_unit_one: {rs(curf.get('unitPattern-count-one', curf.get('unitPattern-count-other')))},",
        f"        currency_unit_other: {rs(curf.get('unitPattern-count-other'))},",
        f"        compact_short: {compact(dec['short']['decimalFormat'])},",
        f"        compact_long: {compact(dec['long']['decimalFormat'])},",
        f"        compact_currency: {compact(curf['short']['standard'])},",
        "        currencies: &[",
    ]
    lines += [f"            {c}," for c in currencies]
    lines += ["        ],", "        units: &["]
    lines += [f"            {u}," for u in unit_rows]
    lines += [
        "        ],",
        f"        per_patterns: [{', '.join(rs(p) for p in per)}],",
        "    },",
        "    dates: DateData {",
        "        months: [",
        f"            {month_list('format', 'wide')},",
        f"            {month_list('format', 'abbreviated')},",
        f"            {month_list('format', 'narrow')},",
        f"            {month_list('stand-alone', 'wide')},",
        f"            {month_list('stand-alone', 'abbreviated')},",
        f"            {month_list('stand-alone', 'narrow')},",
        "        ],",
        "        days: [",
    ]
    for ctx in ["format", "stand-alone"]:
        for w in ["wide", "abbreviated", "short", "narrow"]:
            lines.append(f"            {day_list(ctx, w)},")
    lines += [
        "        ],",
        "        day_periods: [",
        f"            [{rs(dp['abbreviated']['am'])}, {rs(dp['abbreviated']['pm'])}],",
        f"            [{rs(dp['wide']['am'])}, {rs(dp['wide']['pm'])}],",
        f"            [{rs(dp['narrow']['am'])}, {rs(dp['narrow']['pm'])}],",
        "        ],",
        "        flexible_periods: &[" + ", ".join(flex) + "],",
        "        eras: [",
        f"            [{rs(eras['eraAbbr']['0'])}, {rs(eras['eraAbbr']['1'])}],",
        f"            [{rs(eras['eraNames']['0'])}, {rs(eras['eraNames']['1'])}],",
        f"            [{rs(eras['eraNarrow']['0'])}, {rs(eras['eraNarrow']['1'])}],",
        "        ],",
        f"        date_formats: [{', '.join(rs(fmt(df[k])) for k in ['full', 'long', 'medium', 'short'])}],",
        f"        time_formats: [{', '.join(rs(fmt(tf[k])) for k in ['full', 'long', 'medium', 'short'])}],",
        f"        date_time_formats: [{', '.join(rs(fmt(dtf[k])) for k in ['full', 'long', 'medium', 'short'])}],",
        "        available: &[",
    ]
    lines += [f"            ({rs(k)}, {rs(v)})," for k, v in avail]
    lines += ["        ],", "        intervals: &["]
    lines += [f"            ({rs(a)}, {rs(b)}, {rs(c)})," for a, b, c in intervals]
    lines += [
        "        ],",
        f"        interval_fallback: {rs(dtf['intervalFormats']['intervalFormatFallback'])},",
        "    },",
        "};",
        "",
    ]
    return "\n".join(lines)


if __name__ == "__main__":
    main()
