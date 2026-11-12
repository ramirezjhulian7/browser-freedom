//! Los atajos de teclado de la ventana, en un solo sitio: el modificador primario de cada
//! plataforma (`Cmd` en macOS, `Ctrl` en Windows y Linux), la tabla de atajos del navegador,
//! los de edición (seleccionar todo, copiar, cortar, pegar) y cómo se escribe cada atajo en
//! los menús y las páginas internas.
//!
//! Las funciones reciben la plataforma (`mac`) para que los tests cubran las dos tablas en
//! cualquier sistema; la ventana pasa [`MAC`].
//!
//! En macOS el menú de la aplicación que crea `winit` ya usa `Cmd+H` (ocultar), `Cmd+Opción+H`
//! (ocultar las demás) y `Cmd+Q` (salir), y esas teclas no llegan a la ventana: el historial
//! va en `Cmd+Y`, como en Safari y Chrome para Mac.

use winit::keyboard::{Key, ModifiersState, NamedKey};

/// ¿Se compiló para macOS?
pub const MAC: bool = cfg!(target_os = "macos");

/// El modificador primario de `mac` como estado de `winit`.
pub fn primary_state(mac: bool) -> ModifiersState {
    if mac {
        ModifiersState::SUPER
    } else {
        ModifiersState::CONTROL
    }
}

/// ¿Está pulsado el modificador primario (`Cmd` en macOS, `Ctrl` en el resto)?
pub fn primary(m: ModifiersState, mac: bool) -> bool {
    m.intersects(primary_state(mac))
}

/// ¿Los modificadores hacen de la tecla una orden y no texto? En macOS, `Ctrl` o `Cmd`
/// (`Opción` escribe caracteres: `@` es `Opción+2` en el teclado español); en el resto,
/// `Ctrl` o `Alt`.
pub fn blocks_text(m: ModifiersState, mac: bool) -> bool {
    if mac {
        m.control_key() || m.super_key()
    } else {
        m.control_key() || m.alt_key()
    }
}

/// Un atajo del navegador.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    NewTab,
    NewWindow,
    CloseTab,
    NextTab,
    PrevTab,
    /// La pestaña `n` (1 a 8), o la última con 9.
    Tab(u8),
    FocusBar,
    Panel,
    Overlay,
    Reload,
    DumpLayout,
    Bookmark,
    Find,
    SavePng,
    History,
    Downloads,
    Bookmarks,
    Home,
    Back,
    Forward,
    ZoomIn,
    ZoomOut,
    ZoomReset,
}

impl Action {
    /// Los atajos que siguen funcionando aunque el documento cancele el `keydown` (no son
    /// acciones por defecto del documento).
    pub fn survives_prevent_default(self) -> bool {
        matches!(
            self,
            Action::NewTab
                | Action::CloseTab
                | Action::FocusBar
                | Action::Panel
                | Action::Overlay
                | Action::Reload
                | Action::History
        )
    }
}

/// El atajo del navegador de `key` con los modificadores `m` en la plataforma `mac`.
/// Los que dependen del estado (`Esc` para detener, `F3` sin barra de buscar) los decide la
/// ventana.
pub fn shortcut(key: &Key, m: ModifiersState, mac: bool) -> Option<Action> {
    let p = primary(m, mac);
    let shift = m.shift_key();
    let alt = m.alt_key();
    let ctrl = m.control_key();
    let is = |c: &str, k: &str| c.eq_ignore_ascii_case(k);
    Some(match key {
        Key::Named(named) => match named {
            NamedKey::F5 => Action::Reload,
            NamedKey::F6 => Action::FocusBar,
            NamedKey::F12 => Action::Overlay,
            NamedKey::BrowserBack => Action::Back,
            NamedKey::BrowserForward => Action::Forward,
            // `Ctrl+Tab` cambia de pestaña también en macOS (Safari, Chrome).
            NamedKey::Tab if ctrl && shift => Action::PrevTab,
            NamedKey::Tab if ctrl => Action::NextTab,
            NamedKey::PageDown if ctrl => Action::NextTab,
            NamedKey::PageUp if ctrl => Action::PrevTab,
            NamedKey::Home if alt => Action::Home,
            // En macOS, `Opción+←/→` mueve el cursor por palabras en los campos.
            NamedKey::ArrowLeft if alt && !mac => Action::Back,
            NamedKey::ArrowRight if alt && !mac => Action::Forward,
            _ => return None,
        },
        Key::Character(c) if p => match c.as_str() {
            "+" | "=" => Action::ZoomIn,
            "-" => Action::ZoomOut,
            "0" => Action::ZoomReset,
            d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() => {
                Action::Tab(d.as_bytes()[0] - b'0')
            }
            "[" if mac => Action::Back,
            "]" if mac => Action::Forward,
            c if is(c, "t") => Action::NewTab,
            c if is(c, "n") && !shift => Action::NewWindow,
            c if is(c, "w") => Action::CloseTab,
            c if is(c, "l") => Action::FocusBar,
            c if is(c, "i") => Action::Panel,
            c if is(c, "r") => Action::Reload,
            c if is(c, "d") && shift => Action::DumpLayout,
            c if is(c, "d") => Action::Bookmark,
            c if is(c, "f") => Action::Find,
            c if is(c, "s") => Action::SavePng,
            c if is(c, "h") && shift && mac => Action::Home,
            c if is(c, "y") && mac => Action::History,
            c if is(c, "h") && !mac => Action::History,
            c if is(c, "j") => Action::Downloads,
            c if is(c, "b") && shift => Action::Bookmarks,
            _ => return None,
        },
        _ => return None,
    })
}

/// Un atajo de edición de texto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditCommand {
    SelectAll,
    Copy,
    Cut,
    Paste,
}

impl EditCommand {
    /// La letra del atajo.
    pub fn letter(self) -> &'static str {
        match self {
            EditCommand::SelectAll => "a",
            EditCommand::Copy => "c",
            EditCommand::Cut => "x",
            EditCommand::Paste => "v",
        }
    }
}

/// El atajo de edición de `key` (modificador primario más `A`, `C`, `X` o `V`).
pub fn edit_command(key: &Key, m: ModifiersState, mac: bool) -> Option<EditCommand> {
    let Key::Character(c) = key else {
        return None;
    };
    if !primary(m, mac) {
        return None;
    }
    [
        EditCommand::SelectAll,
        EditCommand::Copy,
        EditCommand::Cut,
        EditCommand::Paste,
    ]
    .into_iter()
    .find(|e| c.eq_ignore_ascii_case(e.letter()))
}

/// Cómo se escribe un atajo en la plataforma `mac`. `spec` usa `Mod` para el modificador
/// primario, `Ctrl`, `Alt` y `Shift` (o `Mayús`) para los demás, separados por `+`; varias
/// combinaciones van separadas por ` · `. En Windows y Linux, `Mod+L` es `Ctrl+L`; en macOS,
/// los símbolos de Apple en su orden (`⌃⌥⇧⌘`) y la tecla: `Mod+Shift+B` es `⇧⌘B`, y
/// `Mod+clic` es `⌘+clic` (una palabra se separa con `+`).
pub fn label(spec: &str, mac: bool) -> String {
    spec.split(" · ")
        .map(|combo| label_one(combo, mac))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn label_one(combo: &str, mac: bool) -> String {
    let mut parts: Vec<&str> = combo.split('+').collect();
    // `Ctrl++` (acercar): la tecla es `+`.
    if combo.ends_with("++") {
        parts.retain(|p| !p.is_empty());
        parts.push("+");
    }
    let Some((&last, mods)) = parts.split_last() else {
        return combo.to_owned();
    };
    let is_mod = |p: &str| matches!(p, "Mod" | "Ctrl" | "Alt" | "Shift" | "Mayús");
    let key = last;
    if !mac {
        let mut out: Vec<&str> = mods
            .iter()
            .map(|&m| if m == "Mod" { "Ctrl" } else { m })
            .collect();
        out.push(key);
        return out.join("+");
    }
    // `Mod+Shift` (con la rueda): solo modificadores, sin tecla.
    let (key, mods): (&str, Vec<&str>) = if is_mod(key) {
        ("", parts.clone())
    } else {
        (key, mods.to_vec())
    };
    let has = |names: &[&str]| mods.iter().any(|m| names.contains(m));
    let mut out = String::new();
    for (names, symbol) in [
        (&["Ctrl"][..], '⌃'),
        (&["Alt"][..], '⌥'),
        (&["Shift", "Mayús"][..], '⇧'),
        (&["Mod"][..], '⌘'),
    ] {
        if has(names) {
            out.push(symbol);
        }
    }
    if !out.is_empty() && key.chars().count() > 3 {
        out.push('+');
    }
    out.push_str(key);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::SmolStr;

    fn ch(c: &str) -> Key {
        Key::Character(SmolStr::new(c))
    }

    const CMD: ModifiersState = ModifiersState::SUPER;
    const CTRL: ModifiersState = ModifiersState::CONTROL;
    const SHIFT: ModifiersState = ModifiersState::SHIFT;
    const ALT: ModifiersState = ModifiersState::ALT;

    /// La tabla, con el modificador primario de cada plataforma.
    const TABLE: &[(&str, bool, Action)] = &[
        ("t", false, Action::NewTab),
        ("n", false, Action::NewWindow),
        ("w", false, Action::CloseTab),
        ("l", false, Action::FocusBar),
        ("i", false, Action::Panel),
        ("r", false, Action::Reload),
        ("d", true, Action::DumpLayout),
        ("d", false, Action::Bookmark),
        ("f", false, Action::Find),
        ("s", false, Action::SavePng),
        ("j", false, Action::Downloads),
        ("b", true, Action::Bookmarks),
        ("1", false, Action::Tab(1)),
        ("9", false, Action::Tab(9)),
        ("=", false, Action::ZoomIn),
        ("+", false, Action::ZoomIn),
        ("-", false, Action::ZoomOut),
        ("0", false, Action::ZoomReset),
    ];

    #[test]
    fn primary_modifier_per_platform() {
        for mac in [false, true] {
            let (p, other) = if mac { (CMD, CTRL) } else { (CTRL, CMD) };
            for &(k, shift, action) in TABLE {
                let m = if shift { p | SHIFT } else { p };
                assert_eq!(shortcut(&ch(k), m, mac), Some(action), "{k} mac={mac}");
                // El otro modificador no es un atajo del navegador.
                let m = if shift { other | SHIFT } else { other };
                assert_eq!(shortcut(&ch(k), m, mac), None, "{k} con el otro, mac={mac}");
            }
            // Sin modificador, las letras son texto.
            assert_eq!(shortcut(&ch("t"), ModifiersState::empty(), mac), None);
            assert_eq!(shortcut(&ch("n"), p | SHIFT, mac), None);
        }
    }

    #[test]
    fn platform_specific_shortcuts() {
        // Historial: `Cmd+Y` en macOS (`Cmd+H` es del sistema); `Ctrl+H` en el resto.
        assert_eq!(shortcut(&ch("y"), CMD, true), Some(Action::History));
        assert_eq!(shortcut(&ch("h"), CMD, true), None);
        assert_eq!(shortcut(&ch("h"), CTRL, false), Some(Action::History));
        assert_eq!(shortcut(&ch("y"), CTRL, false), None);
        // Atrás y adelante.
        let left = Key::Named(NamedKey::ArrowLeft);
        let right = Key::Named(NamedKey::ArrowRight);
        assert_eq!(shortcut(&left, ALT, false), Some(Action::Back));
        assert_eq!(shortcut(&right, ALT, false), Some(Action::Forward));
        assert_eq!(shortcut(&left, ALT, true), None);
        assert_eq!(shortcut(&ch("["), CMD, true), Some(Action::Back));
        assert_eq!(shortcut(&ch("]"), CMD, true), Some(Action::Forward));
        assert_eq!(shortcut(&ch("["), CTRL, false), None);
        // Inicio.
        let home = Key::Named(NamedKey::Home);
        for mac in [false, true] {
            assert_eq!(shortcut(&home, ALT, mac), Some(Action::Home));
        }
        assert_eq!(shortcut(&ch("h"), CMD | SHIFT, true), Some(Action::Home));
        // `Ctrl+Tab` en todas.
        let tab = Key::Named(NamedKey::Tab);
        for mac in [false, true] {
            assert_eq!(shortcut(&tab, CTRL, mac), Some(Action::NextTab));
            assert_eq!(shortcut(&tab, CTRL | SHIFT, mac), Some(Action::PrevTab));
            assert_eq!(
                shortcut(&Key::Named(NamedKey::F5), ModifiersState::empty(), mac),
                Some(Action::Reload)
            );
        }
    }

    #[test]
    fn edit_commands_and_text() {
        for mac in [false, true] {
            let (p, other) = if mac { (CMD, CTRL) } else { (CTRL, CMD) };
            for (k, e) in [
                ("a", EditCommand::SelectAll),
                ("C", EditCommand::Copy),
                ("x", EditCommand::Cut),
                ("v", EditCommand::Paste),
            ] {
                assert_eq!(edit_command(&ch(k), p, mac), Some(e));
                assert_eq!(edit_command(&ch(k), other, mac), None);
            }
            assert!(blocks_text(CTRL, mac));
        }
        // En macOS `Cmd` es una orden y `Opción` escribe; en el resto, `Alt` no escribe (la
        // tecla Windows no cambia nada, como antes).
        assert!(blocks_text(CMD, true));
        assert!(!blocks_text(ALT, true));
        assert!(blocks_text(ALT, false));
    }

    #[test]
    fn labels() {
        assert_eq!(label("Mod+L", false), "Ctrl+L");
        assert_eq!(label("Mod+L", true), "⌘L");
        assert_eq!(label("Mod+Shift+B", false), "Ctrl+Shift+B");
        assert_eq!(label("Mod+Shift+B", true), "⇧⌘B");
        assert_eq!(
            label("Mod++ · Mod+− · Mod+0", false),
            "Ctrl++ · Ctrl+− · Ctrl+0"
        );
        assert_eq!(label("Mod++ · Mod+− · Mod+0", true), "⌘+ · ⌘− · ⌘0");
        assert_eq!(label("Ctrl+Tab · Mod+1…9", true), "⌃Tab · ⌘1…9");
        assert_eq!(
            label("Clic central · Mod+clic", true),
            "Clic central · ⌘+clic"
        );
        assert_eq!(label("Mayús+Espacio", true), "⇧+Espacio");
        assert_eq!(label("Mayús+Espacio", false), "Mayús+Espacio");
        assert_eq!(label("F12", true), "F12");
        assert_eq!(label("Mod+Shift", false), "Ctrl+Shift");
        assert_eq!(label("Mod+Shift", true), "⇧⌘");
    }
}
