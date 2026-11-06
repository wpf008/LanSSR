use anyhow::{Result, ensure};

use crate::grid::Grid;

/// Data-frame intervals offered by the desktop system settings.
pub const FRAME_INTERVAL_OPTIONS_MS: [u64; 3] = [30, 50, 100];

/// Default frame interval balances capture time and transfer speed.
pub const DEFAULT_FRAME_INTERVAL_MS: u64 = 50;

/// Built-in transfer parameters; desktop controls can change watermark and timing.
#[derive(Clone, Debug)]
pub struct Config {
    pub width: u32,
    pub height: u32,
    pub cell_size: u32,
    pub interval_ms: u64,
    pub water_mask: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            width: 1440,
            height: 800,
            cell_size: 4,
            interval_ms: DEFAULT_FRAME_INTERVAL_MS,
            water_mask: true,
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        self.grid()?;
        ensure!(
            self.width / self.cell_size >= 2 && self.height / self.cell_size >= 2,
            "Calibration requires at least two rows and two columns of cells"
        );
        ensure!(
            (1..=60_000).contains(&self.interval_ms),
            "interval must be between 1 and 60000 milliseconds"
        );
        Ok(())
    }

    pub fn grid(&self) -> Result<Grid> {
        Grid::new(self.width, self.height, self.cell_size, self.water_mask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_defaults_match_the_transfer_protocol() {
        let config = Config::default();
        config.validate().unwrap();
        assert_eq!(config.width, 1440);
        assert_eq!(config.height, 800);
        assert_eq!(config.cell_size, 4);
        assert_eq!(config.interval_ms, DEFAULT_FRAME_INTERVAL_MS);
        assert!(config.water_mask);
        assert_eq!(config.grid().unwrap().payload_capacity(), 35_994);
        let plain = Config {
            water_mask: false,
            ..config
        };
        plain.validate().unwrap();
        assert_eq!(plain.grid().unwrap().payload_capacity(), 35_994);
    }

    #[test]
    fn desktop_interval_choices_include_the_default_and_are_valid() {
        assert_eq!(FRAME_INTERVAL_OPTIONS_MS, [30, 50, 100]);
        assert!(FRAME_INTERVAL_OPTIONS_MS.contains(&DEFAULT_FRAME_INTERVAL_MS));
        for interval_ms in FRAME_INTERVAL_OPTIONS_MS {
            Config {
                interval_ms,
                ..Config::default()
            }
            .validate()
            .unwrap();
        }
    }

    #[test]
    fn rejects_invalid_geometry_and_frame_interval() {
        for config in [
            Config {
                width: 0,
                ..Config::default()
            },
            Config {
                width: 719,
                ..Config::default()
            },
            Config {
                cell_size: 0,
                ..Config::default()
            },
            Config {
                interval_ms: 0,
                ..Config::default()
            },
            Config {
                interval_ms: 60_001,
                ..Config::default()
            },
            Config {
                cell_size: 3,
                water_mask: true,
                ..Config::default()
            },
        ] {
            assert!(config.validate().is_err(), "{config:?}");
        }
    }
}
