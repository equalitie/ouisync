use super::*;

#[test]
fn serialize_basic() {
    let root = StateMonitor::make_root();
    let child = root.make_child("foo");
    let _value = child.make_value("bar", 42);

    assert_eq!(
        serde_json::to_string(&root).unwrap(),
        r#"{"values":{},"children":["foo:0"]}"#
    );

    assert_eq!(
        serde_json::to_string(&child).unwrap(),
        r#"{"values":{"bar":"42"},"children":[]}"#
    );
}

#[test]
fn serialize_in_insertion_order() {
    {
        let root = StateMonitor::make_root();
        let _a = root.make_value("a", 0);
        let _b = root.make_value("b", 1);
        let _c = root.make_value("c", 2);

        assert_eq!(
            serde_json::to_string(&root).unwrap(),
            r#"{"values":{"a":"0","b":"1","c":"2"},"children":[]}"#,
        );
    }

    {
        let root = StateMonitor::make_root();
        let _a = root.make_value("a", 0);
        let _c = root.make_value("c", 2);
        let _b = root.make_value("b", 1);

        assert_eq!(
            serde_json::to_string(&root).unwrap(),
            r#"{"values":{"a":"0","c":"2","b":"1"},"children":[]}"#,
        );
    }

    {
        let root = StateMonitor::make_root();
        let _c = root.make_value("c", 2);
        let _b = root.make_value("b", 1);
        let _a = root.make_value("a", 0);

        assert_eq!(
            serde_json::to_string(&root).unwrap(),
            r#"{"values":{"c":"2","b":"1","a":"0"},"children":[]}"#,
        );
    }
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
