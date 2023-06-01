use super::*;

#[test]
fn serialize() {
    let root = StateMonitor::make_root();
    let child = root.make_child("foo");
    let _value = child.make_value("bar", 42);

    assert_eq!(
        serde_json::to_string(&root).unwrap(),
        "{\"values\":{},\"children\":[\"foo:0\"]}"
    );

    assert_eq!(
        serde_json::to_string(&child).unwrap(),
        "{\"values\":{\"bar\":\"42\"},\"children\":[]}"
    );
}

#[test]
fn test_parse_monitor_id() {
    let id: MonitorId = "foo".parse().unwrap();
    assert_eq!(id.name, "foo");
    assert_eq!(id.disambiguator, 0);

    let id: MonitorId = "bar:0".parse().unwrap();
    assert_eq!(id.name, "bar");
    assert_eq!(id.disambiguator, 0);

    let id: MonitorId = "baz:1".parse().unwrap();
    assert_eq!(id.name, "baz");
    assert_eq!(id.disambiguator, 1);

    let id: MonitorId = "foo:bar:2".parse().unwrap();
    assert_eq!(id.name, "foo:bar");
    assert_eq!(id.disambiguator, 2);

    assert!("baz:".parse::<MonitorId>().is_err());
    assert!("baz:qux".parse::<MonitorId>().is_err());
}
