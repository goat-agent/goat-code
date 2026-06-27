use std::path::Path;

use goat_tool::{Tool, ToolContext};

#[tokio::test]
#[ignore = "requires a real Chrome install and drives the persistent profile"]
async fn navigates_clicks_debug_evals_and_closes() {
    let tool = goat_tool_browser::browser_tool();
    let ctx = ToolContext::new(Path::new(".")).unwrap();

    let nav = r#"{"action":"navigate","url":"data:text/html,<button onclick='window.__c=1'>Go</button>"}"#;
    let snapshot = tool.run(nav, &ctx).await.unwrap();
    let snapshot = snapshot.as_text().expect("navigate returns text");
    assert!(
        snapshot.contains("[ref=s1:e1]"),
        "snapshot should tag the button: {snapshot}"
    );

    tool.run(r#"{"action":"click","ref":"e1"}"#, &ctx)
        .await
        .unwrap();

    let value = tool
        .run(r#"{"action":"debug_eval","js":"window.__c"}"#, &ctx)
        .await
        .unwrap();
    assert_eq!(value.as_text().unwrap().trim(), "1");

    let closed = tool.run(r#"{"action":"close"}"#, &ctx).await.unwrap();
    assert!(closed.as_text().unwrap().contains("closed"));
}
