#[derive(PartialEq, Debug, Copy, Clone)]
pub enum GbMode {
    Classic,
    Color,
    ColorAsClassic,
}

#[derive(PartialEq, Copy, Clone)]
pub enum GbSpeed {
    Single,
    Double,
}

impl GbMode {
    pub(crate) fn to_u8(self) -> u8 {
        match self {
            GbMode::Classic => 0,
            GbMode::Color => 1,
            GbMode::ColorAsClassic => 2,
        }
    }

    pub(crate) fn from_u8(v: u8) -> Result<GbMode, crate::savestate::StateError> {
        match v {
            0 => Ok(GbMode::Classic),
            1 => Ok(GbMode::Color),
            2 => Ok(GbMode::ColorAsClassic),
            _ => Err(crate::savestate::StateError::BadField("GbMode")),
        }
    }
}

impl GbSpeed {
    pub(crate) fn to_u8(self) -> u8 {
        match self {
            GbSpeed::Single => 0,
            GbSpeed::Double => 1,
        }
    }

    pub(crate) fn from_u8(v: u8) -> Result<GbSpeed, crate::savestate::StateError> {
        match v {
            0 => Ok(GbSpeed::Single),
            1 => Ok(GbSpeed::Double),
            _ => Err(crate::savestate::StateError::BadField("GbSpeed")),
        }
    }
}
