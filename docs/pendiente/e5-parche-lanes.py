import re, sys
R = "D:/Personales/Repos/browser-freedom-army-e5/crates/"

def edit(path, old, new, count=1):
    p = R + path
    s = open(p, encoding="utf-8", newline="").read()
    if s.count(old) != count:
        sys.exit(f"{path}: {s.count(old)} matches for {old[:60]!r}")
    s = s.replace(old, new)
    open(p, "w", encoding="utf-8", newline="").write(s)

T = "bf-style/src/properties/types.rs"
edit(T, '    InlineGrid = "inline-grid",\n', '    InlineGrid = "inline-grid",\n    // E5: CSS Grid 3 §2.2.\n    GridLanes = "grid-lanes",\n    InlineGridLanes = "inline-grid-lanes",\n')
edit(T, "                | Display::InlineGrid\n                | Display::InlineTable\n", "                | Display::InlineGrid\n                | Display::InlineGridLanes\n                | Display::InlineTable\n")
edit(T, "            Display::InlineGrid => Display::Grid,\n", "            Display::InlineGrid => Display::Grid,\n            Display::InlineGridLanes => Display::GridLanes,\n")
edit(T, '            ["inline", "grid"] | ["grid", "inline"] => Display::InlineGrid,\n',
     '            ["inline", "grid"] | ["grid", "inline"] => Display::InlineGrid,\n'
     '            ["block", "grid-lanes"] | ["grid-lanes", "block"] => Display::GridLanes,\n'
     '            ["inline", "grid-lanes"] | ["grid-lanes", "inline"] => Display::InlineGridLanes,\n')

p = R + T
s = open(p, encoding="utf-8", newline="").read()
s += '''
// --- E5: grid lanes (CSS Grid 3) ---

/// E5: eje de rejilla pedido por `grid-lanes-direction` (CSS Grid 3 §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanesAxis {
    Normal,
    Row,
    Column,
}

/// E5: `grid-lanes-direction: normal | [ row | column ] [ fill-reverse || track-reverse ]?`
/// (CSS Grid 3 §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridLanesDirection {
    pub axis: LanesAxis,
    pub fill_reverse: bool,
    pub track_reverse: bool,
}

impl PropValue for GridLanesDirection {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let items = components(input);
        let (first, rest) = items.split_first()?;
        if is_ident(first, "normal") && rest.is_empty() {
            return Some(GridLanesDirection {
                axis: LanesAxis::Normal,
                fill_reverse: false,
                track_reverse: false,
            });
        }
        let axis = if is_ident(first, "row") {
            LanesAxis::Row
        } else if is_ident(first, "column") {
            LanesAxis::Column
        } else {
            return None;
        };
        let (mut fill_reverse, mut track_reverse) = (false, false);
        for v in rest {
            if !fill_reverse && is_ident(v, "fill-reverse") {
                fill_reverse = true;
            } else if !track_reverse && is_ident(v, "track-reverse") {
                track_reverse = true;
            } else {
                return None;
            }
        }
        Some(GridLanesDirection {
            axis,
            fill_reverse,
            track_reverse,
        })
    }
    fn to_css(&self) -> String {
        let mut s = String::from(match self.axis {
            LanesAxis::Normal => "normal",
            LanesAxis::Row => "row",
            LanesAxis::Column => "column",
        });
        if self.fill_reverse {
            s.push_str(" fill-reverse");
        }
        if self.track_reverse {
            s.push_str(" track-reverse");
        }
        s
    }
}

keyword_enum!(
    /// E5: `grid-lanes-pack: normal | dense` (CSS Grid 3 §4.3).
    GridLanesPack {
        Normal = "normal",
        Dense = "dense",
    }
);

/// E5: `flow-tolerance: normal | <length-percentage [0,inf]> | infinite` (CSS Grid 3 §4.2; el
/// borrador actual la llama `fit-tolerance`).
#[derive(Debug, Clone, PartialEq)]
pub enum FlowTolerance {
    Normal,
    LengthPercentage(LengthPercentage),
    Infinite,
}

impl PropValue for FlowTolerance {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] if is_ident(v, "normal") => Some(FlowTolerance::Normal),
            [v] if is_ident(v, "infinite") => Some(FlowTolerance::Infinite),
            [v] => parse_non_negative_lp(v).map(FlowTolerance::LengthPercentage),
            _ => None,
        }
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        match self {
            FlowTolerance::LengthPercentage(lp) => {
                FlowTolerance::LengthPercentage(lp.compute(&ctx.len))
            }
            v => v.clone(),
        }
    }
    fn to_css(&self) -> String {
        match self {
            FlowTolerance::Normal => "normal".into(),
            FlowTolerance::LengthPercentage(lp) => lp.to_string(),
            FlowTolerance::Infinite => "infinite".into(),
        }
    }
}
'''
open(p, "w", encoding="utf-8", newline="").write(s)

edit("bf-style/src/properties/mod.rs",
     '    ContainIntrinsicHeight "contain-intrinsic-height" contain_intrinsic_height: ContainIntrinsicSize = "none", inherited: false, animatable: true;\n',
     '    ContainIntrinsicHeight "contain-intrinsic-height" contain_intrinsic_height: ContainIntrinsicSize = "none", inherited: false, animatable: true;\n'
     '    // E5: grid lanes (CSS Grid 3 §2.3, §4.2 y §4.3).\n'
     '    GridLanesDirection "grid-lanes-direction" grid_lanes_direction: GridLanesDirection = "normal", inherited: false, animatable: false;\n'
     '    GridLanesPack "grid-lanes-pack" grid_lanes_pack: GridLanesPack = "normal", inherited: false, animatable: false;\n'
     '    FlowTolerance "flow-tolerance" flow_tolerance: FlowTolerance = "normal", inherited: false, animatable: true;\n')
edit("bf-style/src/properties/shorthands.rs", '    alias!("-webkit-appearance" => Appearance),\n',
     '    alias!("-webkit-appearance" => Appearance),\n'
     '    // E5: nombres de `flow-tolerance` en otras versiones de CSS Grid 3 (§4.2).\n'
     '    alias!("fit-tolerance" => FlowTolerance),\n'
     '    alias!("item-tolerance" => FlowTolerance),\n')
edit("bf-style/src/cascade.rs",
     "                Display::Flex | Display::InlineFlex | Display::Grid | Display::InlineGrid\n",
     "                Display::Flex\n                    | Display::InlineFlex\n                    | Display::Grid\n                    | Display::InlineGrid\n                    | Display::GridLanes\n                    | Display::InlineGridLanes\n")
edit("bf-layout/src/box_tree.rs",
     "            Display::Flex | Display::InlineFlex | Display::Grid | Display::InlineGrid\n",
     "            Display::Flex\n                | Display::InlineFlex\n                | Display::Grid\n                | Display::InlineGrid\n                | Display::GridLanes\n                | Display::InlineGridLanes\n")
edit("bf-layout/src/block.rs",
     "                | Display::InlineGrid\n                | Display::Table\n",
     "                | Display::InlineGrid\n                | Display::GridLanes\n                | Display::InlineGridLanes\n                | Display::Table\n")
G = "bf-layout/src/grid.rs"
edit(G, "    matches!(b.style.display, Display::Grid | Display::InlineGrid)\n",
     "    matches!(\n        b.style.display,\n        Display::Grid | Display::InlineGrid | Display::GridLanes | Display::InlineGridLanes\n    )\n")
edit(G, "const MAX_LINE: i32 = 1000;\n", "const MAX_LINE: i32 = 1000;\n\n// E5: grid lanes (CSS Grid 3).\nmod lanes;\n")
edit(G, "    if !is_grid_container(b)\n        || b.is_absolutely_positioned()\n",
     "    if !is_grid_container(b)\n        || lanes::is_lanes_container(b)\n        || b.is_absolutely_positioned()\n")
edit(G, "        let cs = &container.style;\n        let mut grid = self.grid_setup(container, Some(cb.width), cb.height);\n",
     "        if lanes::is_lanes_container(container) {\n            return self.lanes_layout(container, cb);\n        }\n        let cs = &container.style;\n        let mut grid = self.grid_setup(container, Some(cb.width), cb.height);\n")
edit(G, "        let cs = &b.style;\n        let grid = self.grid_setup(b, None, None);\n",
     "        if lanes::is_lanes_container(b) {\n            return self.lanes_intrinsic_widths(b);\n        }\n        let cs = &b.style;\n        let grid = self.grid_setup(b, None, None);\n")
print("ok")
