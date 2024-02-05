#[cfg(test)]
mod test {
    use futures::{
        stream::{self, StreamExt, TryStreamExt},
        FutureExt, TryFutureExt,
    };
    use serial_test::serial;
    use std::{sync::Arc, time::Duration};
    use tokio::{self, task::JoinError};

    fn test_data() -> Vec<u8> {
        (1..=10).into_iter().collect()
    }

    /// Simulate performing a get request. Time decreases as id increases to exaggerate order
    /// differences and the effect of the buffer.
    async fn perform_get_request(id: u8) -> anyhow::Result<u8> {
        println!("get {id} starting");
        tokio::time::sleep(Duration::from_millis(100 + 50 / (id as u64))).await;
        println!("get {id} complete");
        Ok(id)
    }

    /// Simulate performing a post request. Time increases as id increases to exaggerate order
    /// differences and the effect of the buffer.
    async fn perform_post_request(id: u8) -> anyhow::Result<()> {
        println!("post {id} starting");
        tokio::time::sleep(Duration::from_millis(100 + 5 * id as u64)).await;
        println!("post {id} complete");
        Ok(())
    }

    fn transform_data(result: anyhow::Result<u8>) -> anyhow::Result<u8> {
        match result {
            Ok(id) => {
                println!("transforming {id}");
                Ok(id)
            }
            Err(error) => Err(error),
        }
    }

    fn flatten_join_result<T, E: From<JoinError>>(
        result: Result<Result<T, E>, JoinError>,
    ) -> Result<T, E> {
        match result {
            Ok(ok) => ok,
            Err(error) => Err(error.into()),
        }
    }

    async fn time_test(f: impl std::future::Future<Output = ()>) {
        let start = tokio::time::Instant::now();
        f.await;
        let end = tokio::time::Instant::now();
        let duration = end - start;
        println!("Duration: {}", humantime::format_duration(duration))
    }

    #[tokio::test]
    #[serial]
    pub async fn test_1_naive() {
        time_test(async {
            stream::iter(test_data())
                .then(perform_get_request)
                .map(transform_data)
                .try_for_each(perform_post_request)
                .await
                .unwrap();
        })
        .await;
    }

    #[tokio::test]
    #[serial]
    pub async fn test_2_with_buffer() {
        time_test(async {
            stream::iter(test_data())
                .map(perform_get_request)
                .map(Ok)
                .try_buffer_unordered(2)
                .map(transform_data)
                .try_for_each(perform_post_request)
                .await
                .unwrap()
        })
        .await;
    }

    #[tokio::test]
    #[serial]
    pub async fn test_3_with_buffer_task() {
        time_test(async {
            stream::iter(test_data())
                .map(perform_get_request)
                .map(tokio::spawn)
                .map(Ok)
                .try_buffer_unordered(2)
                // Unwrap task join result
                .map(|result| match result {
                    Ok(ok) => ok,
                    Err(error) => Err(error.into()),
                })
                .map(transform_data)
                .try_for_each(perform_post_request)
                .await
                .unwrap()
        })
        .await;
    }

    #[tokio::test]
    #[serial]
    pub async fn test_4_with_bounded_channel() {
        time_test(async {
            let (tx, rx) = tokio::sync::mpsc::channel(2);
            let get_stream = async move {
                // This is in its own async block so that it and tx gets dropped
                // when the stream completes.
                stream::iter(test_data())
                    .then(perform_get_request)
                    .map(transform_data)
                    .try_for_each(|data| tx.send(data).map_err(anyhow::Error::from))
                    .await
            };

            let post_stream = tokio_stream::wrappers::ReceiverStream::from(rx)
                .map(Ok)
                .try_for_each(perform_post_request);

            tokio::try_join!(get_stream, post_stream,).unwrap();
        })
        .await;
    }

    #[tokio::test]
    #[serial]
    pub async fn test_5_with_bounded_channel_tasks() {
        time_test(async {
            let (tx, rx) = tokio::sync::mpsc::channel(2);

            let get_stream = tokio::spawn(async move {
                // This is in its own async block so that it and tx gets dropped
                // when the stream completes.
                stream::iter(test_data())
                    .then(perform_get_request)
                    .map(transform_data)
                    .try_for_each(|data| tx.send(data).map_err(anyhow::Error::from))
                    .await
            })
            .map(flatten_join_result);

            let post_stream = tokio::spawn(
                tokio_stream::wrappers::ReceiverStream::from(rx)
                    .map(Ok)
                    .try_for_each(perform_post_request),
            )
            .map(flatten_join_result);

            tokio::try_join!(get_stream, post_stream,).unwrap();
        })
        .await;
    }

    #[tokio::test]
    #[serial]
    pub async fn test_7_all_together() {
        time_test(async {
            stream::iter(test_data())
                .map(Ok)
                .try_for_each_concurrent(2, |data| async move {
                    let get_data = perform_get_request(data).await?;
                    let post_data = transform_data(Ok(get_data))?;
                    perform_post_request(post_data).await
                })
                .await
                .unwrap()
        })
        .await;
    }

    #[ignore] // because it deadlocks
    #[tokio::test]
    #[serial]
    pub async fn test_8_std_mutex_await_lock() {
        let mutex = Arc::new(std::sync::Mutex::new(()));
        let sleep_mutex = mutex.clone();
        tokio::join!(
            async move {
                let _guard = sleep_mutex.lock().unwrap();
                tokio::time::sleep(Duration::from_millis(100)).await;
                // _guard held across await until it goes out of scope here
            },
            async move {
                let _guard = mutex.lock().unwrap();
            }
        );
    }

    #[tokio::test]
    #[serial]
    pub async fn test_9_tokio_mutex_await_lock() {
        let mutex = Arc::new(tokio::sync::Mutex::new(()));
        let sleep_mutex = mutex.clone();
        tokio::join!(
            async move {
                let _guard = sleep_mutex.lock().await;
                tokio::time::sleep(Duration::from_millis(100)).await;
                // _guard held across await until it goes out of scope here
            },
            async move {
                let _guard = mutex.lock().await;
            }
        );
    }

    #[ignore] // because it deadlocks
    #[tokio::test]
    #[serial]
    pub async fn test_10_dashmap_async_deadlock() {
        let map: Arc<dashmap::DashMap<&'static str, u32>> = Arc::new(dashmap::DashMap::new());
        let sleep_map = map.clone();
        tokio::join!(
            async move {
                let entry = sleep_map.entry("key");
                let _ref = entry.insert(32);
                tokio::time::sleep(Duration::from_millis(100)).await;
                // _ref held across await until it goes out of scope here
            },
            async move {
                map.get("key");
            }
        );
    }

    #[test]
    #[serial]
    pub fn test_11_dashmap_thread() {
        let map: Arc<dashmap::DashMap<&'static str, u32>> = Arc::new(dashmap::DashMap::new());
        let sleep_map = map.clone();
        let join = std::thread::spawn(move || {
            let entry = sleep_map.entry("key");
            let _ref = entry.insert(32);
            std::thread::sleep(Duration::from_millis(100));
            // _ref goes out of scope here.
        });

        // Wait for thread to acquire lock and begin sleeping.
        std::thread::sleep(Duration::from_millis(10));
        map.get("key");
        join.join().unwrap();
    }

    #[ignore] // because it deadlocks
    #[test]
    #[serial]
    pub fn test_12_dashmap_deadlock() {
        let map: dashmap::DashMap<&'static str, u32> = dashmap::DashMap::new();
        let entry = map.entry("key");
        let _ref = entry.insert(32);
        map.get("key");
        // _ref goes out of scope here
    }
}
