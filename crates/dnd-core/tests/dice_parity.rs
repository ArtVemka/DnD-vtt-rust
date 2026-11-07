//! Replays `fixtures/dice.json`, generated from the JS reference by
//! `tools/gen-dice-fixtures.js`. Any behaviour drift fails here first.

use dnd_core::dice::{parse_formula, roll_formula, Roll};
use serde_json::{json, Value};

const FIXTURES: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/dice.json"
));

fn parsed_json(parsed: &dnd_core::dice::ParsedFormula) -> Value {
    json!({
        "ok": true,
        "source": parsed.source,
        "normalized": parsed.normalized,
        "count": parsed.count,
        "sides": parsed.sides,
        "keep": parsed.keep.map(|keep| json!({
            "mode": keep.mode.as_str(),
            "value": keep.value
        })),
        "modifier": parsed.modifier
    })
}

fn roll_json(roll: &Roll) -> Value {
    json!({
        "formula": roll.formula,
        "count": roll.count,
        "sides": roll.sides,
        "values": roll.values,
        "keptValues": roll.kept_values,
        "droppedValues": roll.dropped_values,
        "modifier": roll.modifier,
        "result": roll.result
    })
}

#[test]
fn dice_matches_js_reference() {
    let cases: Vec<Value> = serde_json::from_str(FIXTURES).expect("fixtures parse");
    assert!(cases.len() > 40, "fixture file looks truncated");

    for case in cases {
        let input = case["input"].as_str().expect("input");
        let parse_expected = &case["parse"];

        match parse_formula(input) {
            Ok(parsed) => {
                assert_eq!(
                    &parsed_json(&parsed),
                    parse_expected,
                    "parse mismatch for {input:?}"
                );
                let roll_expected = &case["roll"];
                assert!(
                    !roll_expected.is_null(),
                    "JS expected a roll for {input:?}"
                );
                let sequence: Vec<u32> = case["sequence"]
                    .as_array()
                    .expect("sequence")
                    .iter()
                    .map(|value| value.as_u64().expect("number") as u32)
                    .collect();
                assert!(!sequence.is_empty(), "empty sequence for {input:?}");
                let mut index = 0usize;
                let roll = roll_formula(&parsed, &mut |_min, _max| {
                    let value = sequence[index % sequence.len()];
                    index += 1;
                    value
                });
                assert_eq!(
                    &roll_json(&roll),
                    roll_expected,
                    "roll mismatch for {input:?}"
                );
            }
            Err(error) => {
                assert_eq!(
                    parse_expected["error"].as_str(),
                    Some(error.0.as_str()),
                    "error mismatch for {input:?}"
                );
            }
        }
    }
}
