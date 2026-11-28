use super::PythonRuntime;
use tempfile::tempdir;

#[tokio::test]
async fn an_append_to_an_existing_data_file_keeps_the_prior_contents() {
    let dir = tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
    std::fs::write(ws.join("data/artifacts/log.jsonl"), "one\ntwo\n").unwrap();
    let runtime = PythonRuntime::new(ws.to_path_buf()).unwrap();

    let staging = ws.join(".lucidos/staging/append-run");
    let out = runtime
        .execute_staged(
            "open('data/artifacts/log.jsonl', 'a').write('three\\n')\nprint('done')",
            vec![],
            &staging,
        )
        .await
        .expect("the append must succeed");
    assert_eq!(out.trim(), "done");

    assert_eq!(
        std::fs::read_to_string(staging.join("data/artifacts/log.jsonl")).unwrap(),
        "one\ntwo\nthree\n",
        "the staged file is what the committer copies over the artifact"
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("data/artifacts/log.jsonl")).unwrap(),
        "one\ntwo\n",
        "the real artifact stays untouched until the engine commits"
    );
}

#[tokio::test]
async fn a_truncating_write_to_an_existing_data_file_still_truncates() {
    let dir = tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
    std::fs::write(ws.join("data/artifacts/report.csv"), "stale,rows\n1,2\n").unwrap();
    let runtime = PythonRuntime::new(ws.to_path_buf()).unwrap();

    let staging = ws.join(".lucidos/staging/truncate-run");
    runtime
        .execute_staged(
            "open('data/artifacts/report.csv', 'w').write('fresh\\n')",
            vec![],
            &staging,
        )
        .await
        .expect("the truncating write must succeed");

    assert_eq!(
        std::fs::read_to_string(staging.join("data/artifacts/report.csv")).unwrap(),
        "fresh\n",
        "a 'w' mode asked for truncation"
    );
}

#[tokio::test]
async fn an_update_mode_open_reads_an_existing_data_file() {
    let dir = tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
    std::fs::write(ws.join("data/artifacts/notes.txt"), "hello\n").unwrap();
    let runtime = PythonRuntime::new(ws.to_path_buf()).unwrap();

    let staging = ws.join(".lucidos/staging/update-run");
    let out = runtime
        .execute_staged(
            "with open('data/artifacts/notes.txt', 'r+') as f:\n    print(f.read().strip())",
            vec![],
            &staging,
        )
        .await
        .expect("r+ on an existing artifact must not raise");

    assert_eq!(out.trim(), "hello");
}
