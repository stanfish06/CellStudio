use std::ops::Range;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dims {
    pub t: u64,
    pub c: u64,
    pub z: u64,
    pub y: u64,
    pub x: u64,
}

impl Dims {
    pub fn as_array(&self) -> [u64; 5] {
        [self.t, self.c, self.z, self.y, self.x]
    }

    pub fn from_array(v: [u64; 5]) -> Self {
        Self {
            t: v[0],
            c: v[1],
            z: v[2],
            y: v[3],
            x: v[4],
        }
    }

    pub fn get(&self, axis: Axis) -> u64 {
        self.as_array()[axis.slot()]
    }

    pub fn voxels(&self) -> u64 {
        self.t * self.c * self.z * self.y * self.x
    }

    pub fn zyx_voxels(&self) -> u64 {
        self.z * self.y * self.x
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    T,
    C,
    Z,
    Y,
    X,
}

impl Axis {
    pub const ALL: [Axis; 5] = [Axis::T, Axis::C, Axis::Z, Axis::Y, Axis::X];

    pub fn slot(self) -> usize {
        match self {
            Axis::T => 0,
            Axis::C => 1,
            Axis::Z => 2,
            Axis::Y => 3,
            Axis::X => 4,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "t" | "time" => Some(Axis::T),
            "c" | "channel" | "ch" => Some(Axis::C),
            "z" | "depth" => Some(Axis::Z),
            "y" => Some(Axis::Y),
            "x" => Some(Axis::X),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Axis::T => "t",
            Axis::C => "c",
            Axis::Z => "z",
            Axis::Y => "y",
            Axis::X => "x",
        }
    }
}

/// Position of each TCZYX axis in the store's own axis list; `None` means the axis is
/// absent from the store and normalizes to extent 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisMap {
    slots: [Option<u8>; 5],
    ndim: u8,
}

impl AxisMap {
    pub fn new(slots: [Option<u8>; 5], ndim: usize) -> Self {
        Self {
            slots,
            ndim: ndim as u8,
        }
    }

    pub fn ndim(&self) -> usize {
        usize::from(self.ndim)
    }

    pub fn slot(&self, axis: Axis) -> Option<usize> {
        self.slots[axis.slot()].map(usize::from)
    }

    /// Store-order shape → TCZYX, absent axes filled with 1.
    pub fn normalize(&self, shape: &[u64]) -> Dims {
        let mut out = [1_u64; 5];
        for axis in Axis::ALL {
            if let Some(i) = self.slot(axis)
                && let Some(extent) = shape.get(i)
            {
                out[axis.slot()] = *extent;
            }
        }
        Dims::from_array(out)
    }

    /// TCZYX region → store-order ranges. Ranges for absent axes are dropped.
    pub fn project(&self, region: &[Range<u64>; 5]) -> Vec<Range<u64>> {
        let mut out = vec![0..1; self.ndim()];
        for axis in Axis::ALL {
            if let Some(i) = self.slot(axis)
                && let Some(slot) = out.get_mut(i)
            {
                *slot = region[axis.slot()].clone();
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    XY,
    XZ,
    YZ,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dtype {
    U8,
    U16,
    U32,
}

impl Dtype {
    pub fn size_bytes(&self) -> usize {
        match self {
            Dtype::U8 => 1,
            Dtype::U16 => 2,
            Dtype::U32 => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PhysicalScale {
    pub z: f64,
    pub y: f64,
    pub x: f64,
}

impl PhysicalScale {
    pub const ISOTROPIC: Self = Self {
        z: 1.0,
        y: 1.0,
        x: 1.0,
    };

    /// Aspect ratio of the orthogonal axis relative to the in-plane axis.
    pub fn ratio(&self, numerator: f64, denominator: f64) -> f64 {
        if denominator.abs() < f64::EPSILON || !numerator.is_finite() || !denominator.is_finite() {
            return 1.0;
        }
        numerator / denominator
    }
}
