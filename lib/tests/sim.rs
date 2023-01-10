//! Integration tests that use simulated environment for repeatable test results

mod common;

fn transfer_large_file() {
    let file_size = 4 * 1024 * 1024;

    /*

    let mut env = Env::with_seed(0);

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    let mut content = vec![0; file_size];
    env.rng.fill(&mut content[..]);

    // Create a file by A and wait until B sees it.
    tracing::info!("writing");
    async {
        let mut file = repo_a.create_file("test.dat").await.unwrap();
        common::write_in_chunks(&mut file, &content, 4096).await;
        file.flush().await.unwrap();
    }
    .instrument(repo_a.span())
    .await;

    tracing::info!("reading");
    common::expect_file_content(&repo_b, "test.dat", &content).await;
    */
}
