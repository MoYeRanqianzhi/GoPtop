//! 棋盘原语 — 棋子、坐标、棋盘容器。
//!
//! 五子棋与围棋在数据层完全一致：棋盘是 `N×N` 的 `Stone` 矩阵，`N` 为 15（五子棋）或 19/13/9（围棋）。
//! 本模块仅提供数据与基础操作（读写、越界检查、遍历），不含任何规则。

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};
use std::fmt;
use std::marker::PhantomData;

/// 棋子颜色，`Empty` 表示空点。黑白之外的状态（如标记）由上层 UI 维护，不进入核心数据。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stone {
    Empty,
    Black,
    White,
}

impl Stone {
    /// 返回对手颜色；`Empty` 的对手仍为 `Empty`（便于无分支调用）。
    #[must_use]
    pub fn opponent(self) -> Self {
        match self {
            Self::Black => Self::White,
            Self::White => Self::Black,
            Self::Empty => Self::Empty,
        }
    }

    /// 是否为空点。
    #[must_use]
    pub fn is_empty(self) -> bool {
        self == Self::Empty
    }
}

/// 棋盘坐标，`(0,0)` 为左上角，`x` 向右、`y` 向下递增，与 SVG/Canvas 坐标系一致。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Coord {
    /// 列索引，`0..N`
    pub x: u8,
    /// 行索引，`0..N`
    pub y: u8,
}

impl Coord {
    /// 构造坐标，不做越界检查（越界由 `Board` 负责）。
    #[must_use]
    pub fn new(x: u8, y: u8) -> Self {
        Self { x, y }
    }
}

/// `N×N` 棋盘，`N` 由 const generic 指定。
///
/// - 五子棋：`Board<15>`
/// - 围棋：`Board<19>`（9/13 路通过 `GameKind::Go { size }` 限制有效区域，物理存储仍为 19×19，避免多尺寸泛型膨胀）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Board<const N: usize> {
    cells: [[Stone; N]; N],
}

// 手动实现 Serialize/Deserialize：serde 对大尺寸 const-generic 数组的 derive 在当前版本下无法推导，
// 改为以 `Vec<Vec<Stone>>` 的形态序列化，避免 `[[Stone; N]; N]: Deserialize` 的 trait 边界问题。
impl<const N: usize> Serialize for Board<N> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("Board", 1)?;
        let rows: Vec<Vec<Stone>> = self.cells.iter().map(|row| row.to_vec()).collect();
        state.serialize_field("cells", &rows)?;
        state.end()
    }
}

impl<'de, const N: usize> Deserialize<'de> for Board<N> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BoardVisitor<const N: usize>(PhantomData<[[Stone; N]; N]>);

        impl<'de, const N: usize> Visitor<'de> for BoardVisitor<N> {
            type Value = Board<N>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "struct Board with {N}x{N} cells")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut cells: Option<Vec<Vec<Stone>>> = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "cells" {
                        cells = Some(map.next_value()?);
                    } else {
                        let _: de::IgnoredAny = map.next_value()?;
                    }
                }
                let rows = cells.ok_or_else(|| de::Error::missing_field("cells"))?;
                if rows.len() != N {
                    return Err(de::Error::custom(format!(
                        "expected {N} rows, got {}",
                        rows.len()
                    )));
                }
                let mut arr = [[Stone::Empty; N]; N];
                for (y, row) in rows.into_iter().enumerate() {
                    if row.len() != N {
                        return Err(de::Error::custom(format!(
                            "expected row len {N}, got {} at row {y}",
                            row.len()
                        )));
                    }
                    for (x, s) in row.into_iter().enumerate() {
                        arr[y][x] = s;
                    }
                }
                Ok(Board { cells: arr })
            }
        }

        deserializer.deserialize_struct("Board", &["cells"], BoardVisitor(PhantomData))
    }
}

impl<const N: usize> Board<N> {
    /// 创建空棋盘。
    #[must_use]
    pub fn new() -> Self {
        Self {
            cells: [[Stone::Empty; N]; N],
        }
    }

    /// 棋盘尺寸。
    #[must_use]
    pub fn size(&self) -> usize {
        N
    }

    /// 读取坐标上的棋子，越界返回 `None`。
    #[must_use]
    pub fn get(&self, c: Coord) -> Option<Stone> {
        if (c.x as usize) < N && (c.y as usize) < N {
            Some(self.cells[c.y as usize][c.x as usize])
        } else {
            None
        }
    }

    /// 在坐标上放置棋子，越界返回 `false`。
    pub fn set(&mut self, c: Coord, stone: Stone) -> bool {
        if (c.x as usize) < N && (c.y as usize) < N {
            self.cells[c.y as usize][c.x as usize] = stone;
            true
        } else {
            false
        }
    }

    /// 坐标是否在棋盘内。
    #[must_use]
    pub fn in_bounds(&self, c: Coord) -> bool {
        (c.x as usize) < N && (c.y as usize) < N
    }

    /// 是否为空点（越界视为非空，避免误落子）。
    #[must_use]
    pub fn is_empty(&self, c: Coord) -> bool {
        self.get(c) == Some(Stone::Empty)
    }

    /// 清空棋盘。
    pub fn clear(&mut self) {
        self.cells = [[Stone::Empty; N]; N];
    }

    /// 遍历所有非空点，调用者可用于渲染或统计。
    pub fn for_each<F>(&self, mut f: F)
    where
        F: FnMut(Coord, Stone),
    {
        for y in 0..N {
            for x in 0..N {
                let s = self.cells[y][x];
                if s != Stone::Empty {
                    f(Coord::new(x as u8, y as u8), s);
                }
            }
        }
    }
}

impl<const N: usize> Default for Board<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// 兼容多尺寸的棋盘枚举，上层 `GameState` 通过它屏蔽 `Board<15>`/`Board<19>` 的泛型差异。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoardVariant {
    /// 15×15，主要用于五子棋。
    B15(Board<15>),
    /// 19×19，围棋全尺寸；9/13 路通过 `GameKind` 的 `size` 限制有效区域。
    B19(Board<19>),
}

impl BoardVariant {
    /// 按尺寸创建空棋盘。
    #[must_use]
    pub fn new(size: usize) -> Self {
        match size {
            15 => Self::B15(Board::new()),
            _ => Self::B19(Board::new()),
        }
    }

    /// 逻辑尺寸（15 或 19）。
    #[must_use]
    pub fn size(&self) -> usize {
        match self {
            Self::B15(_) => 15,
            Self::B19(_) => 19,
        }
    }

    /// 读取坐标上的棋子。
    #[must_use]
    pub fn get(&self, c: Coord) -> Option<Stone> {
        match self {
            Self::B15(b) => b.get(c),
            Self::B19(b) => b.get(c),
        }
    }

    /// 放置棋子。
    pub fn set(&mut self, c: Coord, stone: Stone) -> bool {
        match self {
            Self::B15(b) => b.set(c, stone),
            Self::B19(b) => b.set(c, stone),
        }
    }

    /// 是否为空点。
    #[must_use]
    pub fn is_empty(&self, c: Coord) -> bool {
        self.get(c) == Some(Stone::Empty)
    }

    /// 是否在逻辑范围内（含尺寸限制）。
    #[must_use]
    pub fn in_bounds(&self, c: Coord) -> bool {
        match self {
            Self::B15(b) => b.in_bounds(c),
            Self::B19(b) => b.in_bounds(c),
        }
    }

    /// 清空。
    pub fn clear(&mut self) {
        match self {
            Self::B15(b) => b.clear(),
            Self::B19(b) => b.clear(),
        }
    }
}
