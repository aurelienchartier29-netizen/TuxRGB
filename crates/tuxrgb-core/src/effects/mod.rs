//! Lighting effects.
//!
//! Every effect is described declaratively ([`EffectKind::params`]) so the GUI
//! can build its settings panel automatically, and rendered by a pure
//! function of time, parameters and LED positions ([`render`]).

mod render;

pub use render::{EffectState, FrameContext, render};

use crate::color::{Harmony, Palette, Rgb};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EffectKind {
    Static,
    Breathing,
    RainbowWave,
    ColorCycle,
    GradientWave,
    Aurora,
    Starfield,
    AudioVisualizer,
    ScreenSync,
    // TuxRGB 2.0 – native versions of the OpenRGB Effects / Hardware Sync
    // plugin effects.
    Plasma,
    Fire,
    Comet,
    Scanner,
    Rain,
    Bubbles,
    Swirl,
    Lightning,
    Shader,
    HardwareMonitor,
}

/// What an effect needs from the engine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Requirements {
    /// Must be re-rendered every frame.
    pub animated: bool,
    pub audio: bool,
    pub screen: bool,
    /// CPU / GPU / RAM sensors (Hardware Sync).
    pub hardware: bool,
    /// GPU-rendered GLSL shader frame.
    pub shader: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Ambient,
    Animated,
    Reactive,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ambient => "Ambiance",
            Self::Animated => "Animés",
            Self::Reactive => "Réactifs",
        }
    }
}

const ANIMATED: Requirements = Requirements {
    animated: true,
    audio: false,
    screen: false,
    hardware: false,
    shader: false,
};

impl EffectKind {
    pub const ALL: [EffectKind; 19] = [
        EffectKind::Static,
        EffectKind::Breathing,
        EffectKind::Aurora,
        EffectKind::Plasma,
        EffectKind::RainbowWave,
        EffectKind::ColorCycle,
        EffectKind::GradientWave,
        EffectKind::Starfield,
        EffectKind::Comet,
        EffectKind::Scanner,
        EffectKind::Rain,
        EffectKind::Bubbles,
        EffectKind::Swirl,
        EffectKind::Fire,
        EffectKind::Lightning,
        EffectKind::Shader,
        EffectKind::AudioVisualizer,
        EffectKind::ScreenSync,
        EffectKind::HardwareMonitor,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Static => "Statique",
            Self::Breathing => "Respiration",
            Self::RainbowWave => "Vague arc-en-ciel",
            Self::ColorCycle => "Cycle de couleurs",
            Self::GradientWave => "Vague dégradée",
            Self::Aurora => "Aurore",
            Self::Starfield => "Ciel étoilé",
            Self::AudioVisualizer => "Visualiseur audio",
            Self::ScreenSync => "Synchro écran",
            Self::Plasma => "Plasma",
            Self::Fire => "Feu",
            Self::Comet => "Comète",
            Self::Scanner => "Scanner",
            Self::Rain => "Pluie",
            Self::Bubbles => "Bulles",
            Self::Swirl => "Tourbillon",
            Self::Lightning => "Orage",
            Self::Shader => "Shader GLSL",
            Self::HardwareMonitor => "Moniteur matériel",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Static => "Une couleur unie sur tout le setup.",
            Self::Breathing => "La couleur s'allume et s'éteint en douceur.",
            Self::RainbowWave => "Un arc-en-ciel qui traverse physiquement votre setup.",
            Self::ColorCycle => "Tout le setup parcourt le spectre à l'unisson.",
            Self::GradientWave => "Des couleurs qui ondulent les unes vers les autres.",
            Self::Aurora => "Des voiles lumineux lents, façon aurore boréale.",
            Self::Starfield => "Des étoiles qui scintillent sur un fond nocturne.",
            #[cfg(not(windows))]
            Self::AudioVisualizer => "Réagit à la musique jouée par le PC (PipeWire).",
            #[cfg(windows)]
            Self::AudioVisualizer => "Réagit à la musique jouée par le PC (WASAPI).",
            Self::ScreenSync => "Chaque LED reprend la zone de l'écran qu'elle occupe.",
            Self::Plasma => "Un champ de couleurs liquide qui ondule lentement.",
            Self::Fire => "Des flammes qui montent depuis le bas de chaque appareil.",
            Self::Comet => "Une comète et sa traînée traversent le setup.",
            Self::Scanner => "Un faisceau qui balaie le setup d'un bord à l'autre.",
            Self::Rain => "Des gouttes qui tombent à travers tout le setup.",
            Self::Bubbles => "Des ondes circulaires qui éclosent un peu partout.",
            Self::Swirl => "Les couleurs tournent autour du centre du setup.",
            Self::Lightning => "Des éclairs aléatoires sur un ciel d'orage.",
            Self::Shader => "Un shader GLSL rendu par la carte graphique.",
            Self::HardwareMonitor => "La température et la charge du PC en couleurs.",
        }
    }

    pub fn category(self) -> Category {
        match self {
            Self::Static | Self::Breathing | Self::Aurora | Self::Plasma => Category::Ambient,
            Self::AudioVisualizer | Self::ScreenSync | Self::HardwareMonitor => Category::Reactive,
            _ => Category::Animated,
        }
    }

    pub fn requirements(self) -> Requirements {
        match self {
            Self::Static => Requirements::default(),
            Self::AudioVisualizer => Requirements {
                audio: true,
                ..ANIMATED
            },
            Self::ScreenSync => Requirements {
                screen: true,
                ..ANIMATED
            },
            Self::HardwareMonitor => Requirements {
                hardware: true,
                ..ANIMATED
            },
            Self::Shader => Requirements {
                shader: true,
                ..ANIMATED
            },
            _ => ANIMATED,
        }
    }

    /// Representative colours for the effect thumbnail in the gallery.
    pub fn thumbnail(self) -> &'static [Rgb] {
        const RAINBOW: &[Rgb] = &[
            Rgb::new(255, 59, 48),
            Rgb::new(255, 204, 0),
            Rgb::new(52, 199, 89),
            Rgb::new(0, 199, 190),
            Rgb::new(10, 132, 255),
            Rgb::new(191, 90, 242),
        ];
        match self {
            Self::Static => const { &[Rgb::new(124, 77, 255), Rgb::new(124, 77, 255)] },
            Self::Breathing => {
                const {
                    &[
                        Rgb::new(20, 12, 40),
                        Rgb::new(0, 180, 255),
                        Rgb::new(20, 12, 40),
                    ]
                }
            }
            Self::RainbowWave | Self::ColorCycle => RAINBOW,
            Self::GradientWave => const { &[Rgb::new(255, 0, 128), Rgb::new(0, 220, 255)] },
            Self::Aurora => {
                const {
                    &[
                        Rgb::new(10, 30, 60),
                        Rgb::new(0, 255, 160),
                        Rgb::new(120, 60, 255),
                    ]
                }
            }
            Self::Starfield => {
                const {
                    &[
                        Rgb::new(5, 8, 30),
                        Rgb::new(255, 255, 220),
                        Rgb::new(5, 8, 30),
                    ]
                }
            }
            Self::AudioVisualizer => {
                const {
                    &[
                        Rgb::new(0, 255, 140),
                        Rgb::new(255, 220, 0),
                        Rgb::new(255, 40, 80),
                    ]
                }
            }
            Self::ScreenSync => {
                const {
                    &[
                        Rgb::new(255, 120, 0),
                        Rgb::new(40, 90, 255),
                        Rgb::new(20, 200, 120),
                    ]
                }
            }
            Self::Plasma => {
                const {
                    &[
                        Rgb::new(255, 0, 170),
                        Rgb::new(90, 40, 255),
                        Rgb::new(0, 220, 255),
                        Rgb::new(255, 0, 170),
                    ]
                }
            }
            Self::Fire => {
                const {
                    &[
                        Rgb::new(255, 230, 120),
                        Rgb::new(255, 110, 0),
                        Rgb::new(180, 20, 0),
                        Rgb::new(30, 0, 0),
                    ]
                }
            }
            Self::Comet => {
                const {
                    &[
                        Rgb::new(4, 6, 20),
                        Rgb::new(0, 90, 160),
                        Rgb::new(160, 240, 255),
                    ]
                }
            }
            Self::Scanner => {
                const {
                    &[
                        Rgb::new(20, 0, 0),
                        Rgb::new(255, 20, 30),
                        Rgb::new(20, 0, 0),
                    ]
                }
            }
            Self::Rain => {
                const {
                    &[
                        Rgb::new(10, 40, 90),
                        Rgb::new(0, 4, 16),
                        Rgb::new(60, 170, 255),
                    ]
                }
            }
            Self::Bubbles => {
                const {
                    &[
                        Rgb::new(255, 80, 200),
                        Rgb::new(10, 10, 30),
                        Rgb::new(0, 220, 200),
                    ]
                }
            }
            Self::Swirl => {
                const {
                    &[
                        Rgb::new(255, 59, 48),
                        Rgb::new(10, 132, 255),
                        Rgb::new(52, 199, 89),
                        Rgb::new(255, 204, 0),
                    ]
                }
            }
            Self::Lightning => {
                const {
                    &[
                        Rgb::new(8, 10, 40),
                        Rgb::new(220, 230, 255),
                        Rgb::new(8, 10, 40),
                    ]
                }
            }
            Self::Shader => {
                const {
                    &[
                        Rgb::new(0, 255, 200),
                        Rgb::new(255, 0, 120),
                        Rgb::new(255, 200, 0),
                    ]
                }
            }
            Self::HardwareMonitor => {
                const {
                    &[
                        Rgb::new(0, 200, 120),
                        Rgb::new(255, 190, 0),
                        Rgb::new(255, 40, 40),
                    ]
                }
            }
        }
    }

    pub fn params(self) -> &'static [ParamSpec] {
        use ParamKind::*;
        const SPEED: ParamSpec = ParamSpec {
            key: "speed",
            label: "Vitesse",
            kind: Range {
                min: 0.05,
                max: 3.0,
                step: 0.05,
            },
            default: ParamDefault::Number(1.0),
        };
        const SCALE: ParamSpec = ParamSpec {
            key: "scale",
            label: "Largeur de vague",
            kind: Range {
                min: 0.2,
                max: 4.0,
                step: 0.1,
            },
            default: ParamDefault::Number(1.0),
        };
        const DIRECTION: ParamSpec = ParamSpec {
            key: "direction",
            label: "Direction",
            kind: Choice(DIRECTIONS),
            default: ParamDefault::Choice("Gauche → droite"),
        };
        // Static & Breathing: follow the desktop accent colour live.
        const KDE_ACCENT: ParamSpec = ParamSpec {
            key: KDE_ACCENT_KEY,
            label: "Synchroniser avec le thème",
            kind: Toggle,
            default: ParamDefault::Toggle(false),
        };
        // Multi-colour effects: colours derived from the desktop accent.
        const PALETTE: ParamSpec = ParamSpec {
            key: SYSTEM_PALETTE_KEY,
            label: "Palette Système Harmonieuse",
            kind: Toggle,
            default: ParamDefault::Toggle(false),
        };
        const HARMONY: ParamSpec = ParamSpec {
            key: HARMONY_KEY,
            label: "Harmonie",
            kind: Choice(HARMONIES),
            default: ParamDefault::Choice("Analogue"),
        };
        const RAINBOW_TOGGLE: ParamSpec = ParamSpec {
            key: "rainbow",
            label: "Arc-en-ciel",
            kind: Toggle,
            default: ParamDefault::Toggle(false),
        };
        const SCOPE: ParamSpec = ParamSpec {
            key: SCOPE_KEY,
            label: "Portée",
            kind: Choice(SCOPES),
            default: ParamDefault::Choice("Chaque appareil"),
        };
        const fn color(key: &'static str, label: &'static str, c: Rgb) -> ParamSpec {
            ParamSpec {
                key,
                label,
                kind: Color,
                default: ParamDefault::Color(c),
            }
        }
        const fn range(
            key: &'static str,
            label: &'static str,
            min: f32,
            max: f32,
            step: f32,
            default: f32,
        ) -> ParamSpec {
            ParamSpec {
                key,
                label,
                kind: Range { min, max, step },
                default: ParamDefault::Number(default),
            }
        }
        match self {
            Self::Static => {
                const {
                    &[
                        KDE_ACCENT,
                        color("color", "Couleur", Rgb::new(124, 77, 255)),
                    ]
                }
            }
            Self::Breathing => {
                const {
                    &[
                        KDE_ACCENT,
                        PALETTE,
                        HARMONY,
                        color("color", "Couleur", Rgb::new(0, 180, 255)),
                        range("speed", "Respirations par minute", 4.0, 60.0, 1.0, 14.0),
                        range("floor", "Luminosité minimale", 0.0, 0.8, 0.05, 0.05),
                    ]
                }
            }
            Self::RainbowWave => const { &[SPEED, SCALE, DIRECTION] },
            Self::ColorCycle => const { &[PALETTE, HARMONY, SPEED] },
            Self::GradientWave => {
                const {
                    &[
                        PALETTE,
                        HARMONY,
                        color("color_a", "Couleur A", Rgb::new(255, 0, 128)),
                        color("color_b", "Couleur B", Rgb::new(0, 220, 255)),
                        SPEED,
                        SCALE,
                        DIRECTION,
                    ]
                }
            }
            Self::Aurora => {
                const {
                    &[
                        PALETTE,
                        HARMONY,
                        color("color_a", "Voile principal", Rgb::new(0, 255, 160)),
                        color("color_b", "Voile secondaire", Rgb::new(120, 60, 255)),
                        color("background", "Ciel", Rgb::new(4, 10, 30)),
                        range("speed", "Vitesse", 0.05, 2.0, 0.05, 0.4),
                    ]
                }
            }
            Self::Starfield => {
                const {
                    &[
                        PALETTE,
                        HARMONY,
                        color("background", "Fond", Rgb::new(4, 6, 24)),
                        color("star", "Étoiles", Rgb::new(255, 244, 214)),
                        range("density", "Densité", 0.05, 1.0, 0.05, 0.35),
                        range("speed", "Scintillement", 0.1, 3.0, 0.1, 1.0),
                        ParamSpec {
                            key: "rainbow",
                            label: "Étoiles multicolores",
                            kind: Toggle,
                            default: ParamDefault::Toggle(false),
                        },
                    ]
                }
            }
            Self::AudioVisualizer => {
                const {
                    &[
                        ParamSpec {
                            key: "mode",
                            label: "Style",
                            kind: Choice(AUDIO_MODES),
                            default: ParamDefault::Choice("Spectre sur chaque appareil"),
                        },
                        PALETTE,
                        HARMONY,
                        ParamSpec {
                            key: "palette",
                            label: "Couleurs",
                            kind: Choice(&["Arc-en-ciel", "Dégradé", "Couleur d'accent"]),
                            default: ParamDefault::Choice("Arc-en-ciel"),
                        },
                        color("color_a", "Couleur basse", Rgb::new(0, 255, 140)),
                        color("color_b", "Couleur haute", Rgb::new(255, 40, 80)),
                        range("sensitivity", "Sensibilité", 0.2, 3.0, 0.1, 1.0),
                        color("background", "Fond au repos", Rgb::new(6, 4, 18)),
                    ]
                }
            }
            Self::ScreenSync => {
                const {
                    &[
                        ParamSpec {
                            key: "mode",
                            label: "Correspondance",
                            kind: Choice(&[
                                "Position sur le canevas",
                                "Bords de l'écran",
                                "Couleur moyenne",
                            ]),
                            default: ParamDefault::Choice("Position sur le canevas"),
                        },
                        range(
                            "saturation",
                            "Renfort des couleurs capturées",
                            0.5,
                            2.5,
                            0.1,
                            1.3,
                        ),
                        range("smoothing", "Lissage", 0.0, 0.95, 0.05, 0.5),
                        range(
                            "black_level",
                            "Ignorer les zones sombres",
                            0.0,
                            0.3,
                            0.01,
                            0.04,
                        ),
                    ]
                }
            }
            Self::Plasma => {
                const {
                    &[
                        PALETTE,
                        HARMONY,
                        range("speed", "Vitesse", 0.05, 2.0, 0.05, 0.5),
                        range("scale", "Taille des motifs", 0.3, 3.0, 0.1, 1.0),
                    ]
                }
            }
            Self::Fire => {
                const {
                    &[
                        PALETTE,
                        HARMONY,
                        SCOPE,
                        range("speed", "Vitesse", 0.2, 3.0, 0.1, 1.0),
                        range("height", "Hauteur des flammes", 0.2, 1.5, 0.05, 0.8),
                    ]
                }
            }
            Self::Comet => {
                const {
                    &[
                        PALETTE,
                        HARMONY,
                        color("color", "Couleur", Rgb::new(120, 220, 255)),
                        RAINBOW_TOGGLE,
                        color("background", "Fond", Rgb::new(2, 4, 14)),
                        SPEED,
                        range("tail", "Longueur de la traînée", 0.05, 1.0, 0.05, 0.35),
                        DIRECTION,
                    ]
                }
            }
            Self::Scanner => {
                const {
                    &[
                        PALETTE,
                        HARMONY,
                        color("color", "Couleur", Rgb::new(255, 20, 30)),
                        color("background", "Fond", Rgb::new(12, 0, 2)),
                        SPEED,
                        range("width", "Largeur du faisceau", 0.03, 0.5, 0.01, 0.12),
                        DIRECTION,
                    ]
                }
            }
            Self::Rain => {
                const {
                    &[
                        PALETTE,
                        HARMONY,
                        color("color", "Gouttes", Rgb::new(60, 170, 255)),
                        color("background", "Fond", Rgb::new(0, 3, 12)),
                        SPEED,
                        range("density", "Densité", 0.1, 1.0, 0.05, 0.5),
                    ]
                }
            }
            Self::Bubbles => {
                const {
                    &[
                        PALETTE,
                        HARMONY,
                        RAINBOW_TOGGLE,
                        color("color", "Couleur", Rgb::new(255, 80, 200)),
                        color("background", "Fond", Rgb::new(4, 2, 14)),
                        SPEED,
                        range("count", "Nombre de bulles", 1.0, 10.0, 1.0, 4.0),
                        range("width", "Épaisseur", 0.02, 0.3, 0.01, 0.08),
                    ]
                }
            }
            Self::Swirl => {
                const {
                    &[
                        PALETTE,
                        HARMONY,
                        SPEED,
                        range("twist", "Torsion", 0.0, 4.0, 0.1, 1.2),
                    ]
                }
            }
            Self::Lightning => {
                const {
                    &[
                        PALETTE,
                        HARMONY,
                        color("color", "Éclairs", Rgb::new(210, 225, 255)),
                        color("background", "Ciel", Rgb::new(4, 6, 26)),
                        range("rate", "Fréquence", 0.1, 3.0, 0.1, 0.8),
                    ]
                }
            }
            Self::Shader => {
                const {
                    &[
                        ParamSpec {
                            key: "shader",
                            label: "Shader",
                            kind: Choice(SHADER_PRESETS),
                            default: ParamDefault::Choice("Néon liquide"),
                        },
                        SPEED,
                        range("smoothing", "Lissage", 0.0, 0.9, 0.05, 0.2),
                    ]
                }
            }
            Self::HardwareMonitor => {
                const {
                    &[
                        ParamSpec {
                            key: "sensor",
                            label: "Capteur",
                            kind: Choice(SENSORS),
                            default: ParamDefault::Choice("Automatique (par appareil)"),
                        },
                        ParamSpec {
                            key: "display",
                            label: "Affichage",
                            kind: Choice(&["Couleur unie", "Jauge"]),
                            default: ParamDefault::Choice("Couleur unie"),
                        },
                        color("cold", "Froid / faible", Rgb::new(0, 200, 120)),
                        color("warm", "Moyen", Rgb::new(255, 190, 0)),
                        color("hot", "Chaud / élevé", Rgb::new(255, 30, 30)),
                        range("temp_low", "Température basse (°C)", 20.0, 70.0, 1.0, 45.0),
                        range(
                            "temp_high",
                            "Température haute (°C)",
                            50.0,
                            105.0,
                            1.0,
                            85.0,
                        ),
                    ]
                }
            }
        }
    }

    /// The effect can follow the desktop accent colour (single colour).
    pub fn supports_kde_accent(self) -> bool {
        matches!(self, Self::Static | Self::Breathing)
    }

    /// The effect can use the "Palette Système Harmonieuse".
    pub fn supports_system_palette(self) -> bool {
        self.params().iter().any(|p| p.key == SYSTEM_PALETTE_KEY)
    }

    /// The palette is enabled for this effect.
    pub fn uses_system_palette(self, params: &EffectParams) -> bool {
        self.supports_system_palette() && params.toggle(self, SYSTEM_PALETTE_KEY)
    }

    /// Parameters replaced by the desktop colours while the system palette
    /// (or the accent synchronisation) is enabled: the GUI shows them as
    /// "provided by the system" instead of a picker.
    pub fn overridden(self, params: &EffectParams, key: &str) -> bool {
        let palette = self.uses_system_palette(params);
        let accent = self.supports_kde_accent() && params.toggle(self, KDE_ACCENT_KEY);
        match key {
            "color" if self.supports_kde_accent() => accent || palette,
            "color" | "color_a" | "color_b" | "background" | "star" => palette,
            "rainbow" => palette,
            "palette" => palette,
            HARMONY_KEY => !palette,
            _ => false,
        }
    }

    /// The system palette of this effect, from the desktop accent.
    pub fn system_palette(self, params: &EffectParams, accent: Rgb) -> Palette {
        let harmony = match params.choice(self, HARMONY_KEY) {
            1 => Harmony::Complementary,
            2 => Harmony::Triadic,
            _ => Harmony::Analogous,
        };
        Palette::from_accent(accent, harmony)
    }

    /// Colour of Static / Breathing: the chosen colour or the desktop
    /// accent. Saturation and brightness are applied later, globally, by
    /// the engine.
    pub fn main_color(self, params: &EffectParams, accent: Rgb) -> Rgb {
        if self.supports_kde_accent() && params.toggle(self, KDE_ACCENT_KEY) {
            accent
        } else {
            params.color(self, "color")
        }
    }

    /// Accent colour used by the GUI when this effect is active.
    pub fn primary_color(self, params: &EffectParams) -> Option<Rgb> {
        match self {
            Self::Static | Self::Breathing | Self::Comet | Self::Scanner | Self::Rain => {
                Some(params.color(self, "color"))
            }
            Self::GradientWave | Self::Aurora => Some(params.color(self, "color_a")),
            _ => None,
        }
    }
}

/// Legacy per-effect saturation key (TuxRGB ≤ 1.3); the saturation is now
/// global (0–200 %) and the old value is migrated by the application.
pub const SATURATION_KEY: &str = "saturation";

/// Parameter key of the "Synchroniser avec le thème" switch.
pub const KDE_ACCENT_KEY: &str = "kde_accent";

/// Parameter key of the "Palette Système Harmonieuse" switch.
pub const SYSTEM_PALETTE_KEY: &str = "system_palette";

/// Parameter key of the palette harmony choice.
pub const HARMONY_KEY: &str = "harmony";

/// Parameter key of the "per device / whole setup" choice.
pub const SCOPE_KEY: &str = "scope";

pub const HARMONIES: &[&str] = &["Analogue", "Complémentaire", "Triadique"];

pub const SCOPES: &[&str] = &["Chaque appareil", "Tout le setup"];

pub const AUDIO_MODES: &[&str] = &[
    "Spectre sur chaque appareil",
    "Spectre sur le setup",
    "Pulsation",
    "Onde centrale",
    "Flash sur les basses",
];

pub const SENSORS: &[&str] = &[
    "Automatique (par appareil)",
    "Température CPU",
    "Charge CPU",
    "Température GPU",
    "Charge GPU",
    "Mémoire utilisée",
];

/// Built-in GLSL shaders (see the shader renderer); "Personnalisé" loads
/// `~/.config/tuxrgb/shaders/custom.frag`.
pub const SHADER_PRESETS: &[&str] = &[
    "Néon liquide",
    "Tunnel",
    "Nébuleuse",
    "Vagues océan",
    "Personnalisé",
];

pub const DIRECTIONS: &[&str] = &[
    "Gauche → droite",
    "Droite → gauche",
    "Haut → bas",
    "Bas → haut",
    "Diagonale",
    "Depuis le centre",
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamKind {
    Color,
    Range { min: f32, max: f32, step: f32 },
    Choice(&'static [&'static str]),
    Toggle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamDefault {
    Color(Rgb),
    Number(f32),
    Choice(&'static str),
    Toggle(bool),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: ParamKind,
    pub default: ParamDefault,
}

/// A parameter value as stored in the configuration file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Toggle(bool),
    Number(f32),
    Color(Rgb),
    Choice(String),
}

impl From<ParamDefault> for ParamValue {
    fn from(d: ParamDefault) -> Self {
        match d {
            ParamDefault::Color(c) => Self::Color(c),
            ParamDefault::Number(n) => Self::Number(n),
            ParamDefault::Choice(s) => Self::Choice(s.to_owned()),
            ParamDefault::Toggle(b) => Self::Toggle(b),
        }
    }
}

/// User values for every effect, keyed by effect then parameter.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EffectParams(pub BTreeMap<EffectKind, BTreeMap<String, ParamValue>>);

impl EffectParams {
    fn spec(kind: EffectKind, key: &str) -> Option<&'static ParamSpec> {
        kind.params().iter().find(|p| p.key == key)
    }

    fn get(&self, kind: EffectKind, key: &str) -> ParamValue {
        self.0
            .get(&kind)
            .and_then(|m| m.get(key))
            .cloned()
            .or_else(|| Self::spec(kind, key).map(|s| s.default.into()))
            .unwrap_or(ParamValue::Number(0.0))
    }

    pub fn set(&mut self, kind: EffectKind, key: &str, value: ParamValue) {
        self.0
            .entry(kind)
            .or_default()
            .insert(key.to_owned(), value);
    }

    pub fn reset(&mut self, kind: EffectKind) {
        self.0.remove(&kind);
    }

    pub fn value(&self, kind: EffectKind, key: &str) -> ParamValue {
        self.get(kind, key)
    }

    pub fn color(&self, kind: EffectKind, key: &str) -> Rgb {
        match self.get(kind, key) {
            ParamValue::Color(c) => c,
            ParamValue::Choice(s) => Rgb::from_hex(&s).unwrap_or(Rgb::WHITE),
            _ => Rgb::WHITE,
        }
    }

    pub fn number(&self, kind: EffectKind, key: &str) -> f32 {
        let v = match self.get(kind, key) {
            ParamValue::Number(n) => n,
            ParamValue::Toggle(b) => b as u8 as f32,
            _ => 0.0,
        };
        match Self::spec(kind, key).map(|s| s.kind) {
            Some(ParamKind::Range { min, max, .. }) => v.clamp(min, max),
            _ => v,
        }
    }

    pub fn toggle(&self, kind: EffectKind, key: &str) -> bool {
        matches!(self.get(kind, key), ParamValue::Toggle(true))
    }

    /// Index of the selected choice (0 when unknown).
    pub fn choice(&self, kind: EffectKind, key: &str) -> usize {
        let ParamValue::Choice(selected) = self.get(kind, key) else {
            return 0;
        };
        match Self::spec(kind, key).map(|s| s.kind) {
            Some(ParamKind::Choice(options)) => {
                options.iter().position(|o| *o == selected).unwrap_or(0)
            }
            _ => 0,
        }
    }

    pub fn choice_label(&self, kind: EffectKind, key: &str) -> &'static str {
        match Self::spec(kind, key).map(|s| s.kind) {
            Some(ParamKind::Choice(options)) => {
                options[self.choice(kind, key).min(options.len() - 1)]
            }
            _ => "",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_returned() {
        let p = EffectParams::default();
        assert_eq!(p.color(EffectKind::Static, "color"), Rgb::new(124, 77, 255));
        assert_eq!(p.number(EffectKind::Breathing, "speed"), 14.0);
        assert_eq!(p.choice(EffectKind::RainbowWave, "direction"), 0);
    }

    #[test]
    fn values_roundtrip_through_toml() {
        let mut p = EffectParams::default();
        p.set(
            EffectKind::Static,
            "color",
            ParamValue::Color(Rgb::new(1, 2, 3)),
        );
        p.set(EffectKind::RainbowWave, "speed", ParamValue::Number(2.5));
        p.set(
            EffectKind::RainbowWave,
            "direction",
            ParamValue::Choice("Diagonale".into()),
        );
        p.set(EffectKind::Starfield, "rainbow", ParamValue::Toggle(true));

        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            params: EffectParams,
        }
        let s = toml::to_string(&Wrapper { params: p.clone() }).unwrap();
        let back: Wrapper = toml::from_str(&s).unwrap();
        assert_eq!(back.params, p);
        assert_eq!(back.params.choice(EffectKind::RainbowWave, "direction"), 4);
    }

    #[test]
    fn every_effect_has_valid_defaults() {
        for kind in EffectKind::ALL {
            for spec in kind.params() {
                if let (ParamKind::Choice(opts), ParamDefault::Choice(d)) =
                    (spec.kind, spec.default)
                {
                    assert!(opts.contains(&d), "{kind:?}.{}", spec.key);
                }
            }
        }
    }
}
