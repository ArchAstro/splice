//! Light leaks: soft washes of warm or cool light drifting across the
//! frame, laid over footage. Each is a see-through scene (`motion::Spec`),
//! so it renders like any other scene and can be edited like one.

/// A light leak: its name, the colours its tile shows, and its scene.
pub struct Leak {
    pub name: &'static str,
    pub colors: [u32; 2],
    pub scene: &'static str,
}

pub const LEAKS: &[Leak] = &[
    Leak {
        name: "Warm leak",
        colors: [0xff7a3d, 0xffc46b],
        scene: r##"{
  "layers": [
    { "type": "ellipse", "name": "glow", "x": [[0, "0%"], [4, "35%", "inOutSine"]], "y": "30%", "width": "110%", "height": "170%",
      "fill": { "radial": true, "stops": [[0, "#ff6a2bcc"], [0.35, "#ff6a2b77"], [0.7, "#ff6a2b22"], [1, "#ff6a2b00"]] },
      "opacity": [[0, 0], [0.9, 1], [3.1, 0.8], [4, 0, "inSine"]] },
    { "type": "ellipse", "name": "flare", "x": [[0, "20%"], [4, "60%", "inOutSine"]], "y": "85%", "width": "70%", "height": "90%",
      "fill": { "radial": true, "stops": [[0, "#ffc46bcc"], [0.35, "#ffc46b77"], [0.7, "#ffc46b22"], [1, "#ffc46b00"]] },
      "opacity": [[0, 0], [1.3, 0.85], [4, 0, "inSine"]] }
  ]
}"##,
    },
    Leak {
        name: "Sun flare",
        colors: [0xffe7a8, 0xfff8e8],
        scene: r##"{
  "layers": [
    { "type": "ellipse", "name": "sun", "x": [[0, "88%"], [4, "72%", "inOutSine"]], "y": "12%", "width": "90%", "height": "120%",
      "fill": { "radial": true, "stops": [[0, "#ffe7a8cc"], [0.35, "#ffe7a877"], [0.7, "#ffe7a822"], [1, "#ffe7a800"]] },
      "opacity": [[0, 0], [0.9, 1], [3.1, 0.8], [4, 0, "inSine"]] },
    { "type": "ellipse", "name": "streak", "x": [[0, "70%"], [4, "45%", "inOutSine"]], "y": [[0, "38%"], [4, "58%"]], "width": "140%", "height": "24%", "rotate": -24,
      "fill": { "radial": true, "stops": [[0, "#fff8e8cc"], [0.35, "#fff8e877"], [0.7, "#fff8e822"], [1, "#fff8e800"]] },
      "opacity": [[0, 0], [1.3, 0.85], [4, 0, "inSine"]] }
  ]
}"##,
    },
    Leak {
        name: "Cool leak",
        colors: [0x6c9ef8, 0xc792ea],
        scene: r##"{
  "layers": [
    { "type": "ellipse", "name": "glow", "x": [[0, "100%"], [4, "65%", "inOutSine"]], "y": "65%", "width": "110%", "height": "170%",
      "fill": { "radial": true, "stops": [[0, "#5b8cffcc"], [0.35, "#5b8cff77"], [0.7, "#5b8cff22"], [1, "#5b8cff00"]] },
      "opacity": [[0, 0], [0.9, 1], [3.1, 0.8], [4, 0, "inSine"]] },
    { "type": "ellipse", "name": "tint", "x": [[0, "80%"], [4, "40%", "inOutSine"]], "y": "15%", "width": "70%", "height": "90%",
      "fill": { "radial": true, "stops": [[0, "#c792eacc"], [0.35, "#c792ea77"], [0.7, "#c792ea22"], [1, "#c792ea00"]] },
      "opacity": [[0, 0], [1.3, 0.85], [4, 0, "inSine"]] }
  ]
}"##,
    },
];

/// The leak called `name` (in any case, with or without "leak").
pub fn find(name: &str) -> Option<&'static Leak> {
    let want = name.trim().to_lowercase().replace(['-', '_'], " ");
    let want = want.trim_end_matches(" leak");
    (!want.is_empty())
        .then(|| LEAKS.iter().find(|l| l.name.to_lowercase().starts_with(want)))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_leak_is_a_valid_scene() {
        for leak in LEAKS {
            let spec = crate::motion::Spec::parse(leak.scene);
            assert!(spec.is_ok(), "{}: {:?}", leak.name, spec.err());
        }
        assert_eq!(find("warm").map(|l| l.name), Some("Warm leak"));
        assert_eq!(find("sun-flare").map(|l| l.name), Some("Sun flare"));
        assert_eq!(find("Cool").map(|l| l.name), Some("Cool leak"));
        assert!(find("").is_none());
    }
}
