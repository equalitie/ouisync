use super::*;

#[test]
fn serialize() {
    let root = StateMonitor::make_root();
    let child = root.make_child("foo");
    let _value = child.make_value("bar", 42);

    assert_eq!(
        serde_json::to_string(&root).unwrap(),
        "{\"version\":2,\"values\":{},\"children\":{\"foo:0\":1}}"
    );

    assert_eq!(
        serde_json::to_string(&child).unwrap(),
        "{\"version\":1,\"values\":{\"bar\":\"42\"},\"children\":{}}"
    );
}
