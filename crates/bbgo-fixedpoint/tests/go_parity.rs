use bbgo_fixedpoint::{Decimal, Dnum, Legacy};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixtures {
    parse: Vec<Parse>,
    arithmetic: Vec<Arithmetic>,
}
#[derive(Deserialize)]
struct Parse {
    input: String,
    output: String,
}
#[derive(Deserialize)]
struct Arithmetic {
    a: String,
    b: String,
    add: String,
    sub: String,
    mul: String,
    div: String,
}

fn compare<N: Decimal>(json: &str) {
    let fixtures: Fixtures = serde_json::from_str(json).unwrap();
    for case in fixtures.parse {
        assert_eq!(case.input.parse::<N>().unwrap().to_string(), case.output);
    }
    for case in fixtures.arithmetic {
        let a: N = case.a.parse().unwrap();
        let b: N = case.b.parse().unwrap();
        for (operation, actual, expected) in [
            ("add", a.checked_add(b), case.add),
            ("sub", a.checked_sub(b), case.sub),
            ("mul", a.checked_mul(b), case.mul),
            ("div", a.checked_div(b), case.div),
        ] {
            assert_eq!(
                actual.unwrap().to_string(),
                expected,
                "{} {} {}",
                case.a,
                operation,
                case.b
            );
        }
    }
}
#[test]
fn original_go_default_vectors() {
    compare::<Legacy>(include_str!("fixtures/go-legacy.json"));
}
#[test]
fn original_go_dnum_vectors() {
    compare::<Dnum>(include_str!("fixtures/go-dnum.json"));
}
