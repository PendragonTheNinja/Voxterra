//! Biomes: classified from climate and terrain, never placed (M13 task 2,
//! `docs/biomes.md`).
//!
//! A column's biome is a pure function of what the elevation field and the
//! climate map say about it — [`Surface`] (height, relief, uplift),
//! temperature at its own height and precipitation, each nudged by a little
//! boundary noise so edges wander instead of following the climate grid.
//! Being a pure function of `(x, z)` is what lets the chunk generator, the
//! LOD sampler and the player's readout agree on every column.
//!
//! ## The rules, in order
//!
//! - **Sea** (height ≤ 0): polar where the sea-level temperature freezes the
//!   surface, deep ocean off the shelf, warm or temperate shallow seas.
//! - **Coasts**: land within [`SHORE_HEIGHT`] of sea level — rocky where the
//!   region is rough, shingle where cold, sand otherwise; ice sheets reach
//!   the sea uninterrupted.
//! - **Mountains** (uplift a real share of the height): rock and snow above
//!   the snowline, alpine meadow above the treeline.
//! - **Cold**: ice sheet, then tundra.
//! - **Dry**, by Köppen's own threshold — a climate is arid when annual
//!   precipitation is under `20·T + 140` mm (`T` the mean annual temperature
//!   in °C): desert below half of it, steppe below all of it. Warm, dry and
//!   rough is badlands.
//! - **Humid**, by temperature and then wetness: boreal forest; tropical
//!   rainforest, dry forest, savanna; Mediterranean scrub where warm and
//!   only moderately wet; temperate rainforest where very wet; temperate
//!   forest.
//!
//! Thresholds are in the game's climate units (ADR-0017), which are tuned to
//! Earth's, so the numbers read as Earth's do.

use crate::climate::{SNOW_LINE_C, temperature_at};
use crate::elevation::{MAX_UPLIFT, Surface};

/// Every biome in the catalogue (`docs/biomes.md`), numbered as there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Biome {
    TropicalRainforest,
    TropicalDryForest,
    Savanna,
    HotDesert,
    HotSteppe,
    Mediterranean,
    TemperateGrassland,
    ColdDesert,
    TemperateForest,
    TemperateRainforest,
    BorealForest,
    Tundra,
    IceSheet,
    AlpineMeadow,
    AlpineRock,
    Badlands,
    SandBeach,
    ShingleBeach,
    RockyCoast,
    WarmShallowSea,
    TemperateSea,
    PolarSea,
    DeepOcean,
    // --- Water (M14, `docs/biomes.md` #24–32): classified from the
    // drainage network, over the climate biome the column would have.
    River,
    Lake,
    /// A closed basin's salt lake or salt flat.
    SaltLake,
    Floodplain,
    Marsh,
    Swamp,
    Bog,
    Delta,
    MangroveCoast,
}

impl Biome {
    /// The whole catalogue, in its order.
    pub const ALL: [Biome; 32] = [
        Biome::TropicalRainforest,
        Biome::TropicalDryForest,
        Biome::Savanna,
        Biome::HotDesert,
        Biome::HotSteppe,
        Biome::Mediterranean,
        Biome::TemperateGrassland,
        Biome::ColdDesert,
        Biome::TemperateForest,
        Biome::TemperateRainforest,
        Biome::BorealForest,
        Biome::Tundra,
        Biome::IceSheet,
        Biome::AlpineMeadow,
        Biome::AlpineRock,
        Biome::Badlands,
        Biome::SandBeach,
        Biome::ShingleBeach,
        Biome::RockyCoast,
        Biome::WarmShallowSea,
        Biome::TemperateSea,
        Biome::PolarSea,
        Biome::DeepOcean,
        Biome::River,
        Biome::Lake,
        Biome::SaltLake,
        Biome::Floodplain,
        Biome::Marsh,
        Biome::Swamp,
        Biome::Bog,
        Biome::Delta,
        Biome::MangroveCoast,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Biome::TropicalRainforest => "tropical rainforest",
            Biome::TropicalDryForest => "tropical dry forest",
            Biome::Savanna => "savanna",
            Biome::HotDesert => "hot desert",
            Biome::HotSteppe => "hot steppe",
            Biome::Mediterranean => "Mediterranean scrub",
            Biome::TemperateGrassland => "temperate grassland",
            Biome::ColdDesert => "cold desert",
            Biome::TemperateForest => "temperate forest",
            Biome::TemperateRainforest => "temperate rainforest",
            Biome::BorealForest => "boreal forest",
            Biome::Tundra => "tundra",
            Biome::IceSheet => "ice sheet",
            Biome::AlpineMeadow => "alpine meadow",
            Biome::AlpineRock => "alpine rock and snow",
            Biome::Badlands => "badlands",
            Biome::SandBeach => "sand beach",
            Biome::ShingleBeach => "shingle beach",
            Biome::RockyCoast => "rocky coast",
            Biome::WarmShallowSea => "warm shallow sea",
            Biome::TemperateSea => "temperate sea",
            Biome::PolarSea => "polar sea",
            Biome::DeepOcean => "deep ocean",
            Biome::River => "river",
            Biome::Lake => "freshwater lake",
            Biome::SaltLake => "salt lake or salt flat",
            Biome::Floodplain => "floodplain",
            Biome::Marsh => "marsh",
            Biome::Swamp => "swamp",
            Biome::Bog => "bog",
            Biome::Delta => "delta",
            Biome::MangroveCoast => "mangrove coast",
        }
    }

    /// Under the sea surface.
    pub fn is_sea(self) -> bool {
        matches!(
            self,
            Biome::WarmShallowSea | Biome::TemperateSea | Biome::PolarSea | Biome::DeepOcean
        )
    }

    pub fn is_coast(self) -> bool {
        matches!(
            self,
            Biome::SandBeach | Biome::ShingleBeach | Biome::RockyCoast
        )
    }
}

/// The water biome of a column the drainage network has given water or a
/// lowland (M14 task 4), if any: the column's own body says which, and the
/// climate which kind of wetland — bog where it is cold (the Hudson Bay
/// lowlands, Irish bogs), swamp where it is hot (the Pantanal, the
/// Atchafalaya), marsh between (the Camargue) — and which delta is
/// mangrove (the Sundarbans).
pub fn water_biome(
    body: crate::drainage::Body,
    temperature: f32,
    sea_level_c: f32,
) -> Option<Biome> {
    use crate::drainage::Body;
    Some(match body {
        Body::River(_) => Biome::River,
        Body::Lake => Biome::Lake,
        Body::SaltLake | Body::SaltFlat => Biome::SaltLake,
        Body::Floodplain => Biome::Floodplain,
        Body::Pool | Body::Wetland if temperature < BOG_BELOW_C => Biome::Bog,
        Body::Pool | Body::Wetland if temperature >= SWAMP_FROM_C => Biome::Swamp,
        Body::Pool | Body::Wetland => Biome::Marsh,
        Body::Delta if sea_level_c >= MANGROVE_FROM_C => Biome::MangroveCoast,
        Body::Delta => Biome::Delta,
        Body::Dry => return None,
    })
}

/// A wetland colder than this (mean annual °C) is a bog: peat.
const BOG_BELOW_C: f32 = 3.0;
/// A wetland at least this warm is a swamp.
const SWAMP_FROM_C: f32 = 18.0;
/// A delta whose sea is at least this warm grows mangroves.
const MANGROVE_FROM_C: f32 = 20.0;

/// Land this close to sea level, blocks, is shore.
pub const SHORE_HEIGHT: i64 = 3;
/// Region roughness ([`Surface::relief`]) above which a shore is rocky.
const ROCKY_SHORE_RELIEF: f32 = 0.30;
/// Shingle, not sand: on cold coasts (sea-level °C below this, where
/// glaciers left the gravel) and on rough ones (relief above this, where
/// waves break up cliffs) — the Baltic and the English Channel both.
const SHINGLE_BELOW_C: f32 = 6.0;
const SHINGLE_RELIEF: f32 = 0.18;
/// Sea-level temperature below which the sea surface freezes: sea ice.
const SEA_ICE_BELOW_C: f32 = -2.0;
/// Depth, blocks, beyond which the sea is deep ocean (the shelf ends at ~20
/// and the continental slope falls to ~260).
const DEEP_OCEAN_BELOW: i64 = 120;
/// Sea-level temperature above which a shallow sea is warm (coral seas).
const WARM_SEA_ABOVE_C: f32 = 20.0;

/// Share of `MAX_UPLIFT` from which ground counts as mountain.
const MOUNTAIN_UPLIFT: f32 = 0.06;
/// Mean annual temperature, °C, at the treeline (mountains only).
const TREELINE_C: f32 = 1.0;
/// Mean annual temperature below which lowland is ice sheet, and tundra.
pub(crate) const ICE_SHEET_BELOW_C: f32 = -14.0;
const TUNDRA_BELOW_C: f32 = -5.0;
/// Below this, humid lowland is boreal forest.
const BOREAL_BELOW_C: f32 = 3.0;
/// Tropical climates (Köppen A) at or above this mean temperature.
const TROPICAL_FROM_C: f32 = 20.0;
/// Hot dry climates (BWh, BSh) at or above this, cold (BWk, BSk) below.
const HOT_DRY_FROM_C: f32 = 18.0;
/// Rainforest and dry forest from these totals, mm/yr; savanna below.
const RAINFOREST_FROM_MM: f32 = 2_000.0;
const DRY_FOREST_FROM_MM: f32 = 1_200.0;
/// Mediterranean: warm, and humid by less than this multiple of the
/// threshold.
const MEDITERRANEAN_FROM_C: f32 = 14.0;
const MEDITERRANEAN_WETNESS: f32 = 1.3;
/// Temperate rainforest: wet for its temperature — at least this many times
/// the aridity threshold, and never under this total — and below this
/// temperature. Cool climates evaporate less, so they need less rain:
/// Haida Gwaii is rainforest on ~1 300 mm at 8 °C, Valdivia on ~2 500 at 12.
const TEMPERATE_RAINFOREST_WETNESS: f32 = 4.0;
const TEMPERATE_RAINFOREST_FROM_MM: f32 = 1_300.0;
const TEMPERATE_RAINFOREST_BELOW_C: f32 = 16.0;
/// Badlands: dry, at least this warm, and this rough.
const BADLANDS_FROM_C: f32 = 10.0;
const BADLANDS_RELIEF: f32 = 0.45;

/// How far boundary noise moves the inputs: temperature by up to this many
/// °C, precipitation by up to this share.
pub(crate) const TEMPERATURE_JITTER_C: f32 = 1.5;
pub(crate) const PRECIPITATION_JITTER: f32 = 0.22;

use crate::drainage::Water;

/// A column's climate and biome.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Column {
    pub surface: Surface,
    /// Mean annual temperature at the column's own height, °C (the sea
    /// surface's for seabeds), boundary noise included.
    pub temperature: f32,
    /// Sea-level temperature here, °C, boundary noise included.
    pub sea_level_temperature: f32,
    /// Annual precipitation, mm, boundary noise included.
    pub precipitation: f32,
    pub biome: Biome,
    /// Surface-patch noise in `[-1, 1]`: which of its biome's surfaces this
    /// column takes (`materials`). 0 from [`classify`] alone; the generator
    /// sets it.
    pub patch: f32,
    /// The river, lake or salt flat here (M14), if any. Dry from
    /// [`classify`] alone; the generator sets it.
    pub water: Water,
}

/// Köppen's aridity threshold, mm/yr: the annual precipitation below which a
/// climate of mean temperature `t` °C is dry (rain spread through the year).
pub fn aridity_threshold(t: f32) -> f32 {
    20.0 * t.max(0.0) + 140.0
}

/// Classify a column from its surface, its climate-map values and its
/// boundary noise (each in `[-1, 1]`).
pub fn classify(
    surface: Surface,
    sea_level_c: f32,
    precip: f32,
    noise: (f32, f32),
    coastal: bool,
) -> Column {
    let sea_level_c = sea_level_c + TEMPERATURE_JITTER_C * noise.0;
    let precipitation = (precip * (1.0 + PRECIPITATION_JITTER * noise.1)).max(0.0);
    let temperature = temperature_at(sea_level_c, surface.height);
    let biome = biome_of(surface, sea_level_c, temperature, precipitation, coastal);
    Column {
        surface,
        temperature,
        sea_level_temperature: sea_level_c,
        precipitation,
        biome,
        patch: 0.0,
        water: Water::DRY,
    }
}

/// `coastal`: whether the sea is near (`DrainageMap::is_coastal`). Sea and
/// coast are only there; low ground inland is land, or a lake's.
fn biome_of(s: Surface, sea_c: f32, t: f32, p: f32, coastal: bool) -> Biome {
    let h = s.height;
    if h <= 0 && coastal {
        return if sea_c < SEA_ICE_BELOW_C {
            Biome::PolarSea
        } else if -h > DEEP_OCEAN_BELOW {
            Biome::DeepOcean
        } else if sea_c >= WARM_SEA_ABOVE_C {
            Biome::WarmShallowSea
        } else {
            Biome::TemperateSea
        };
    }
    if h <= SHORE_HEIGHT && coastal && t >= ICE_SHEET_BELOW_C {
        return if s.relief >= ROCKY_SHORE_RELIEF {
            Biome::RockyCoast
        } else if sea_c < SHINGLE_BELOW_C || s.relief >= SHINGLE_RELIEF {
            Biome::ShingleBeach
        } else {
            Biome::SandBeach
        };
    }
    // Mountains inside an ice-sheet climate are under the ice, as
    // Antarctica's are: ice sheet first, then the alpine zones.
    if t < ICE_SHEET_BELOW_C {
        return Biome::IceSheet;
    }
    if s.uplift >= MOUNTAIN_UPLIFT * MAX_UPLIFT {
        if t < SNOW_LINE_C {
            return Biome::AlpineRock;
        }
        if t < TREELINE_C {
            return Biome::AlpineMeadow;
        }
    }
    if t < TUNDRA_BELOW_C {
        return Biome::Tundra;
    }
    let arid = aridity_threshold(t);
    if p < arid {
        // Badlands are arid ground, not merely dry: desert-level rain.
        if t >= BADLANDS_FROM_C && s.relief >= BADLANDS_RELIEF && p < 0.5 * arid {
            return Biome::Badlands;
        }
        let hot = t >= HOT_DRY_FROM_C;
        return match (p < 0.5 * arid, hot) {
            (true, true) => Biome::HotDesert,
            (true, false) => Biome::ColdDesert,
            (false, true) => Biome::HotSteppe,
            (false, false) => Biome::TemperateGrassland,
        };
    }
    if t < BOREAL_BELOW_C {
        return Biome::BorealForest;
    }
    if t >= TROPICAL_FROM_C {
        return if p >= RAINFOREST_FROM_MM {
            Biome::TropicalRainforest
        } else if p >= DRY_FOREST_FROM_MM {
            Biome::TropicalDryForest
        } else {
            Biome::Savanna
        };
    }
    if t >= MEDITERRANEAN_FROM_C && p < MEDITERRANEAN_WETNESS * arid {
        return Biome::Mediterranean;
    }
    let rainforest_from = (TEMPERATE_RAINFOREST_WETNESS * arid).max(TEMPERATE_RAINFOREST_FROM_MM);
    if p >= rainforest_from && t < TEMPERATE_RAINFOREST_BELOW_C {
        return Biome::TemperateRainforest;
    }
    Biome::TemperateForest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn land(height: i64) -> Surface {
        Surface {
            height,
            relief: 0.1,
            uplift: 0.0,
            sand: 0,
            rock: false,
        }
    }

    fn at(s: Surface, sea_c: f32, p: f32) -> Biome {
        classify(s, sea_c, p, (0.0, 0.0), true).biome
    }

    /// Spot checks against Earth: a place with that climate classifies as
    /// the biome it has.
    #[test]
    fn earth_climates_classify_as_earth_does() {
        let lowland = land(60);
        // Manaus: 27 °C, 2 300 mm.
        let warm = 27.0 + LAPSE * 60.0;
        assert_eq!(at(lowland, warm, 2_300.0), Biome::TropicalRainforest);
        // Serengeti-ish: hot, 800 mm.
        assert_eq!(at(lowland, warm, 800.0), Biome::Savanna);
        // Sahara: hot, 50 mm.
        assert_eq!(at(lowland, warm, 50.0), Biome::HotDesert);
        // Sahel: hot, 450 mm (threshold ~680).
        assert_eq!(at(lowland, warm, 450.0), Biome::HotSteppe);
        // Paris: 12 °C, 640 mm (threshold 380).
        let mild = 12.0 + LAPSE * 60.0;
        assert_eq!(at(lowland, mild, 640.0), Biome::TemperateForest);
        // Seattle's coast forests: 11 °C, 2 500 mm.
        assert_eq!(at(lowland, mild, 2_500.0), Biome::TemperateRainforest);
        // Gobi: 2–5 °C, 50–200 mm (its rain falls in summer, which Köppen
        // judges more harshly; with no seasons here, the even-rain
        // threshold applies, so take a drier year).
        assert_eq!(at(lowland, 4.0 + LAPSE * 60.0, 100.0), Biome::ColdDesert);
        // Great Plains: 10 °C, 450 mm (threshold 340, half 170).
        assert_eq!(
            at(lowland, 10.0 + LAPSE * 60.0, 300.0),
            Biome::TemperateGrassland
        );
        // Seville: 19 °C, 540 mm (threshold 520, ×1.3 = 676) — Köppen Cs.
        // Rome's ~800 mm is Mediterranean only through its dry summer,
        // which an annual-mean classifier cannot see; it reads as forest.
        assert_eq!(
            at(lowland, 19.0 + LAPSE * 60.0, 540.0),
            Biome::Mediterranean
        );
        // Yakutsk: −9 °C → tundra by mean; Arkhangelsk 1 °C, 600 mm → taiga.
        assert_eq!(at(lowland, 1.0 + LAPSE * 60.0, 600.0), Biome::BorealForest);
        assert_eq!(at(lowland, -9.0 + LAPSE * 60.0, 300.0), Biome::Tundra);
        // Antarctic coast inland: −20 °C.
        assert_eq!(at(lowland, -20.0 + LAPSE * 60.0, 100.0), Biome::IceSheet);
    }

    const LAPSE: f32 = crate::climate::LAPSE_PER_BLOCK;

    #[test]
    fn mountains_have_a_treeline_and_a_snowline() {
        let peak = |h| Surface {
            height: h,
            relief: 0.2,
            uplift: 0.5 * MAX_UPLIFT,
            sand: 0,
            rock: false,
        };
        // Mid-latitude: sea level 12 °C. Snow from ~470.
        assert_eq!(at(peak(600), 12.0, 1_000.0), Biome::AlpineRock);
        assert_eq!(at(peak(400), 12.0, 1_000.0), Biome::AlpineMeadow);
        assert_eq!(at(peak(150), 12.0, 1_000.0), Biome::TemperateForest);
    }

    #[test]
    fn shores_and_seas() {
        assert_eq!(at(land(2), 20.0, 800.0), Biome::SandBeach);
        assert_eq!(at(land(2), 0.0, 800.0), Biome::ShingleBeach);
        let rough = Surface {
            relief: 0.5,
            ..land(2)
        };
        assert_eq!(at(rough, 20.0, 800.0), Biome::RockyCoast);
        let choppy = Surface {
            relief: 0.22,
            ..land(2)
        };
        assert_eq!(
            at(choppy, 20.0, 800.0),
            Biome::ShingleBeach,
            "rough, warm shore"
        );
        assert_eq!(
            at(land(2), -25.0, 100.0),
            Biome::IceSheet,
            "ice reaches the sea"
        );
        assert_eq!(at(land(-10), 25.0, 0.0), Biome::WarmShallowSea);
        assert_eq!(at(land(-10), 12.0, 0.0), Biome::TemperateSea);
        assert_eq!(at(land(-500), 12.0, 0.0), Biome::DeepOcean);
        assert_eq!(at(land(-500), -10.0, 0.0), Biome::PolarSea);
    }

    #[test]
    fn dry_rough_and_warm_is_badlands() {
        let rough = Surface {
            relief: 0.5,
            ..land(200)
        };
        assert_eq!(at(rough, 25.0, 100.0), Biome::Badlands);
        // Merely semi-arid rough ground is steppe, not badlands.
        assert_eq!(at(rough, 25.0, 500.0), Biome::HotSteppe);
    }

    #[test]
    fn the_catalogue_is_complete_and_named() {
        assert_eq!(Biome::ALL.len(), 32);
        for (k, b) in Biome::ALL.iter().enumerate() {
            assert_eq!(*b as usize, k, "ALL is in declaration order");
            assert!(!b.name().is_empty());
        }
        assert_eq!(Biome::ALL.iter().filter(|b| b.is_sea()).count(), 4);
        assert_eq!(Biome::ALL.iter().filter(|b| b.is_coast()).count(), 3);
    }
}
