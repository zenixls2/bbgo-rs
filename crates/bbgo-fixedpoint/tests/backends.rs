use bbgo_fixedpoint::{Decimal, Dnum, Error, Legacy, Value};

fn contract<N: Decimal>() {
    let a: N = "0.1".parse().unwrap();
    let b: N = "0.2".parse().unwrap();
    assert_eq!(a.checked_add(b).unwrap(), "0.3".parse().unwrap());
    assert_eq!(a.checked_mul(b).unwrap(), "0.02".parse().unwrap());
    assert_eq!(a.checked_div(b).unwrap(), "0.5".parse().unwrap());
    assert_eq!(a.checked_div(N::ZERO), Err(Error::DivisionByZero));
    assert_eq!(
        "-1.21".parse::<N>().unwrap().floor_to(a).unwrap(),
        "-1.3".parse().unwrap()
    );
    assert_eq!(a.floor_to(N::ZERO), Err(Error::InvalidStep));
    assert_eq!(a.checked_mul_div(b, b).unwrap(), a);
    let bytes = serde_json::to_vec(&a).unwrap();
    assert_eq!(serde_json::from_slice::<N>(&bytes).unwrap(), a);
    assert!(serde_json::from_str::<N>("0.1").is_err());
    assert!(a.checked_mul_ceil(b).unwrap() >= a.checked_mul(b).unwrap());
}

#[test]
fn statically_dispatched_backends() {
    contract::<Legacy>();
    contract::<Dnum>();
    assert_eq!(
        Value::BACKEND,
        if cfg!(feature = "dnum") {
            "dnum-16"
        } else {
            "legacy-8"
        }
    );
}

#[test]
fn dnum_precision_range_ordering_and_exact_conversions() {
    for s in [
        "1.00000000000000111",
        "1.1e-15",
        "1e-100",
        "1e100",
        "-0",
        "12.5%",
        ".01",
    ] {
        let v: Dnum = s.parse().unwrap();
        assert_eq!(v.to_string().parse::<Dnum>().unwrap(), v);
    }
    assert_eq!(
        "1.00000000000000111".parse::<Dnum>().unwrap().to_string(),
        "1.000000000000001"
    );
    assert_eq!(
        "1.1e-15".parse::<Dnum>().unwrap().coefficient(),
        1_100_000_000_000_000
    );
    assert_eq!("1.1e-15".parse::<Dnum>().unwrap().exponent(), -14);
    for bad in [
        "", ".", "inf", "-inf", "NaN", " 1", "1e", "1e999", "1e-999", "1.2.3",
    ] {
        assert!(bad.parse::<Dnum>().is_err(), "{bad}");
    }
    let ordered: Vec<Dnum> = [
        "-1e100", "-2", "-1", "-1e-100", "0", "1e-100", "1", "2", "1e100",
    ]
    .into_iter()
    .map(|s| s.parse().unwrap())
    .collect();
    assert!(ordered.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(
        Legacy::try_from("0.000000001".parse::<Dnum>().unwrap()),
        Err(Error::Precision)
    );
    assert_eq!(
        Dnum::try_from(Legacy::from_raw(i64::MAX)),
        Err(Error::Precision)
    );
    let value: Legacy = "123.456".parse().unwrap();
    assert_eq!(
        Legacy::try_from(Dnum::try_from(value).unwrap()).unwrap(),
        value
    );
    assert!(
        "1e126"
            .parse::<Dnum>()
            .unwrap()
            .checked_mul("100".parse().unwrap())
            .is_err()
    );
    assert!(
        "1e-129"
            .parse::<Dnum>()
            .unwrap()
            .checked_div("100".parse().unwrap())
            .is_err()
    );
}

#[test]
fn conservative_products_and_exact_grid_points() {
    assert_eq!(
        Dnum::parse_exact("1.00000000000000111"),
        Err(Error::Precision)
    );
    assert_eq!(
        Dnum::parse_exact("100000000000000000000.00000000000")
            .unwrap()
            .to_string(),
        "1e20"
    );
    assert!(serde_json::from_str::<Dnum>("\"1.00000000000000111\"").is_err());
    assert_eq!(
        "10".parse::<Dnum>()
            .unwrap()
            .checked_mul_div("-1".parse().unwrap(), "3".parse().unwrap())
            .unwrap()
            .to_string(),
        "-3.333333333333333"
    );
    let a: Dnum = "1.000000000000001".parse().unwrap();
    let rounded = a.checked_mul_ceil(a).unwrap();
    assert_eq!(rounded.to_string(), "1.000000000000003");
    assert_eq!(a.checked_mul_div(a, a).unwrap(), a);
    assert_eq!(
        "-1e-20"
            .parse::<Dnum>()
            .unwrap()
            .floor_to("3e-21".parse().unwrap())
            .unwrap()
            .to_string(),
        "-0.000000000000000000012"
    );
    // The next grid point needs >16 digits; silently rounding up would oversize.
    assert_eq!(
        "1".parse::<Dnum>()
            .unwrap()
            .floor_to("0.00000000000000003".parse().unwrap()),
        Err(Error::Precision)
    );
}
