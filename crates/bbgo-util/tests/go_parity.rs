use bbgo_util::{fnv32, fnv64, mask_key, split_by_length};
use serde::Deserialize;

#[derive(Deserialize)]
struct Vector {
    input: String,
    fnv32: u32,
    fnv64: u64,
    mask: Option<String>,
    budget: usize,
    chunks: Option<Vec<String>>,
}
#[test]
fn original_go_utility_vectors() {
    let vectors: Vec<Vector> = serde_json::from_str(include_str!("fixtures/go-util.json")).unwrap();
    for v in vectors {
        assert_eq!(fnv32(&v.input), v.fnv32);
        assert_eq!(fnv64(&v.input), v.fnv64);
        if let Some(mask) = v.mask {
            assert_eq!(mask_key(&v.input), mask);
        }
        assert_eq!(
            split_by_length(&v.input, v.budget).unwrap(),
            v.chunks.unwrap_or_default()
        );
    }
}
