use std::collections::HashMap;

use axum::async_trait;
use axum::extract::State;
use axum::headers::ContentType;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::TypedHeader;
use futures::{Future, FutureExt};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::value::RawValue;
use serde_json::Value;

use crate::context::RpcContext;
use crate::jsonrpc::error::RpcError;
use crate::jsonrpc::request::{RawParams, RpcRequest};
use crate::jsonrpc::response::{RpcResponse, RpcResult};

#[derive(Clone)]
pub struct RpcRouter {
    context: RpcContext,
    methods: &'static HashMap<&'static str, Box<dyn RpcMethod>>,
    version: &'static str,
}

pub struct RpcRouterBuilder {
    methods: HashMap<&'static str, Box<dyn RpcMethod>>,
    version: &'static str,
}

impl RpcRouterBuilder {
    pub fn register<I, O, S, M: IntoRpcMethod<'static, I, O, S>>(
        mut self,
        method_name: &'static str,
        method: M,
    ) -> Self {
        self.methods
            .insert(method_name, IntoRpcMethod::into_method(method));
        self
    }

    pub fn build(self, context: RpcContext) -> RpcRouter {
        // Intentionally leak the hashmap to give it a static lifetime.
        //
        // Since the router is expected to be long lived, this shouldn't be an issue.
        let methods = Box::new(self.methods);
        let methods = Box::leak(methods);

        RpcRouter {
            context,
            methods,
            version: self.version,
        }
    }

    fn new(version: &'static str) -> Self {
        RpcRouterBuilder {
            methods: Default::default(),
            version,
        }
    }
}

impl RpcRouter {
    pub fn builder(version: &'static str) -> RpcRouterBuilder {
        RpcRouterBuilder::new(version)
    }

    /// Parses and executes a request. Returns [None] if its a notification.
    async fn run_request<'a>(&self, request: &'a str) -> Option<RpcResponse<'a>> {
        let Ok(request) = serde_json::from_str::<RpcRequest<'_>>(request) else {
            return Some(RpcResponse::INVALID_REQUEST);
        };

        // Ignore notification requests.
        if request.id.is_notification() {
            return None;
        }

        // Also grab the method_name as it is a static str, which is required by the metrics.
        let Some((&method_name, method)) = self.methods.get_key_value(request.method.as_ref())
        else {
            return Some(RpcResponse::method_not_found(request.id));
        };

        metrics::increment_counter!("rpc_method_calls_total", "method" => method_name, "version" => self.version);

        let method = method.invoke(self.context.clone(), request.params);
        let result = std::panic::AssertUnwindSafe(method).catch_unwind().await;

        let output = match result {
            Ok(output) => output,
            Err(_e) => {
                tracing::warn!(method=%request.method, "RPC method panic'd");
                Err(RpcError::InternalError(anyhow::anyhow!("Internal error")))
            }
        };

        if output.is_err() {
            metrics::increment_counter!("rpc_method_calls_failed_total", "method" => method_name, "version" => self.version);
        }

        Some(RpcResponse {
            output,
            id: request.id,
        })
    }
}

#[axum::debug_handler]
pub async fn rpc_handler(
    State(state): State<RpcRouter>,
    TypedHeader(content_type): TypedHeader<ContentType>,
    body: axum::body::Bytes,
) -> impl axum::response::IntoResponse {
    // Only json content allowed.
    if content_type != ContentType::json() {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }

    // Unfortunately due to this https://github.com/serde-rs/json/issues/497
    // we cannot use an enum with borrowed raw values inside to do a single deserialization
    // for us. Instead we have to distinguish manually between a single request and a batch
    // request which we do by checking the first byte.
    if body.as_ref().first() != Some(&b'[') {
        let Ok(request) = serde_json::from_slice::<&RawValue>(&body) else {
            return RpcResponse::PARSE_ERROR.into_response();
        };

        match state.run_request(request.get()).await {
            Some(response) => response.into_response(),
            None => ().into_response(),
        }
    } else {
        let Ok(requests) = serde_json::from_slice::<Vec<&RawValue>>(&body) else {
            return RpcResponse::PARSE_ERROR.into_response();
        };

        if requests.is_empty() {
            return RpcResponse::INVALID_REQUEST.into_response();
        }

        let mut responses = Vec::new();

        for request in requests {
            // Notifications return none and are skipped.
            if let Some(response) = state.run_request(request.get()).await {
                responses.push(response);
            }
        }

        // All requests were notifications.
        if responses.is_empty() {
            return ().into_response();
        }

        serde_json::to_string(&responses).unwrap().into_response()
    }
}

#[axum::async_trait]
pub trait RpcMethod: Send + Sync {
    async fn invoke<'a>(&self, state: RpcContext, input: RawParams<'a>) -> RpcResult;
}

/// Utility trait which automates the serde of an RPC methods input and output.
///
/// This trait is sealed to prevent attempts at implementing it manually. This will
/// likely clash with the existing blanket implementations with very unhelpful error
/// messages.
///
/// This trait is automatically implemented for the following methods:
/// ```
/// async fn input_and_context(ctx: RpcContext, input: impl Deserialize) -> Result<impl Serialize, Into<RpcError>>;
/// async fn input_only(input: impl Deserialize) -> Result<impl Serialize, Into<RpcError>>;
/// async fn context_only(ctx: RpcContext) -> Result<impl Serialize, Into<RpcError>>;
/// ```
///
/// The generics allow us to achieve a form of variadic specilization and can be ignored by callers.
/// See [sealed::Sealed] to add more method signatures or more information on how this works.
pub trait IntoRpcMethod<'a, I, O, S>: sealed::Sealed<I, O, S> {
    fn into_method(self) -> Box<dyn RpcMethod>;
}

impl<'a, T, I, O, S> IntoRpcMethod<'a, I, O, S> for T
where
    T: sealed::Sealed<I, O, S>,
{
    fn into_method(self) -> Box<dyn RpcMethod> {
        sealed::Sealed::<I, O, S>::into_method(self)
    }
}

mod sealed {
    use std::marker::PhantomData;

    use crate::jsonrpc::error::RpcError;

    use super::*;

    /// Sealed implementation of [RpcMethod].
    ///
    /// The generics allow for a form of specialization over a methods Input, Output and State
    /// by treating each as a tuple. Varying the tuple length allows us to target a specific method
    /// signature. This same could be achieved with a single generic but it becomes less clear as
    /// each permuation would require a different tuple length.
    ///
    /// By convention, the lack of a type is equivalent to the unit tuple (). So if we want to target functions
    /// with no input params, no input state and an output:
    /// ```
    /// Sealed<I = (), S = (), O = ((), Ouput)>
    /// ```
    pub trait Sealed<I, O, S> {
        fn into_method(self) -> Box<dyn RpcMethod>;
    }

    /// ```
    /// async fn example(RpcContext, impl Deserialize) -> Result<Output, Into<RpcError>>
    /// ```
    impl<'a, F, Input, Output, Error, Fut> Sealed<((), Input), ((), Output), ((), RpcContext)> for F
    where
        F: Fn(RpcContext, Input) -> Fut + Sync + Send + 'static,
        Input: DeserializeOwned + Send + Sync + 'static,
        Output: Serialize + Send + Sync + 'static,
        Error: Into<RpcError> + Send + Sync + 'static,
        Fut: Future<Output = Result<Output, Error>> + Send,
    {
        fn into_method(self) -> Box<dyn RpcMethod> {
            struct Helper<F, Input, Output, Error> {
                f: F,
                _marker: PhantomData<(Input, Output, Error)>,
            }

            #[axum::async_trait]
            impl<F, Input, Output, Error, Fut> RpcMethod for Helper<F, Input, Output, Error>
            where
                F: Fn(RpcContext, Input) -> Fut + Sync + Send,
                Input: DeserializeOwned + Send + Sync,
                Output: Serialize + Send + Sync,
                Error: Into<RpcError> + Send + Sync,
                Fut: Future<Output = Result<Output, Error>> + Send,
            {
                async fn invoke<'a>(&self, state: RpcContext, input: RawParams<'a>) -> RpcResult {
                    let input = input.deserialize()?;
                    let output = (self.f)(state, input).await.map_err(Into::into)?;
                    serde_json::to_value(output).map_err(|e| RpcError::InternalError(e.into()))
                }
            }

            Box::new(Helper {
                f: self,
                _marker: Default::default(),
            })
        }
    }

    /// ```
    /// async fn example(impl Deserialize) -> Result<Output, Into<RpcError>>
    /// ```
    #[async_trait]
    impl<'a, F, Input, Output, Error, Fut> Sealed<((), Input), ((), Output), ()> for F
    where
        F: Fn(Input) -> Fut + Sync + Send + 'static,
        Input: DeserializeOwned + Sync + Send + 'static,
        Output: Serialize + Sync + Send + 'static,
        Error: Into<RpcError> + Sync + Send + 'static,
        Fut: Future<Output = Result<Output, Error>> + Send,
    {
        fn into_method(self) -> Box<dyn RpcMethod> {
            struct Helper<F, Input, Output, Error> {
                f: F,
                _marker: PhantomData<(Input, Output, Error)>,
            }

            #[axum::async_trait]
            impl<F, Input, Output, Error, Fut> RpcMethod for Helper<F, Input, Output, Error>
            where
                F: Fn(Input) -> Fut + Sync + Send,
                Input: DeserializeOwned + Send + Sync,
                Output: Serialize + Send + Sync,
                Error: Into<RpcError> + Send + Sync,
                Fut: Future<Output = Result<Output, Error>> + Send,
            {
                async fn invoke<'a>(&self, _state: RpcContext, input: RawParams<'a>) -> RpcResult {
                    let input = input.deserialize()?;
                    let output = (self.f)(input).await.map_err(Into::into)?;
                    serde_json::to_value(output).map_err(|e| RpcError::InternalError(e.into()))
                }
            }

            Box::new(Helper {
                f: self,
                _marker: Default::default(),
            })
        }
    }

    /// ```
    /// async fn example(RpcContext) -> Result<Output, Into<RpcError>>
    /// ```
    #[async_trait]
    impl<'a, F, Output, Error, Fut> Sealed<(), ((), Output), ((), RpcContext)> for F
    where
        F: Fn(RpcContext) -> Fut + Sync + Send + 'static,
        Output: Serialize + Sync + Send + 'static,
        Error: Into<RpcError> + Send + Sync + 'static,
        Fut: Future<Output = Result<Output, Error>> + Send,
    {
        fn into_method(self) -> Box<dyn RpcMethod> {
            struct Helper<F, Output, Error> {
                f: F,
                _marker: PhantomData<(Output, Error)>,
            }

            #[axum::async_trait]
            impl<F, Output, Error, Fut> RpcMethod for Helper<F, Output, Error>
            where
                F: Fn(RpcContext) -> Fut + Sync + Send,
                Output: Serialize + Send + Sync,
                Error: Into<RpcError> + Send + Sync,
                Fut: Future<Output = Result<Output, Error>> + Send,
            {
                async fn invoke<'a>(&self, state: RpcContext, input: RawParams<'a>) -> RpcResult {
                    if !input.is_empty() {
                        return Err(RpcError::InvalidParams);
                    }
                    let output = (self.f)(state).await.map_err(Into::into)?;
                    serde_json::to_value(output).map_err(|e| RpcError::InternalError(e.into()))
                }
            }

            Box::new(Helper {
                f: self,
                _marker: Default::default(),
            })
        }
    }

    /// ```
    /// async fn example() -> Result<Output, Into<RpcError>>
    /// ```
    #[async_trait]
    impl<'a, F, Output, Error, Fut> Sealed<(), (), ((), Output)> for F
    where
        F: Fn() -> Fut + Sync + Send + 'static,
        Output: Serialize + Sync + Send + 'static,
        Error: Into<RpcError> + Sync + Send + 'static,
        Fut: Future<Output = Result<Output, Error>> + Send,
    {
        fn into_method(self) -> Box<dyn RpcMethod> {
            struct Helper<F, Output, Error> {
                f: F,
                _marker: PhantomData<(Output, Error)>,
            }

            #[axum::async_trait]
            impl<F, Output, Error, Fut> RpcMethod for Helper<F, Output, Error>
            where
                F: Fn() -> Fut + Sync + Send,
                Output: Serialize + Send + Sync,
                Error: Into<RpcError> + Send + Sync,
                Fut: Future<Output = Result<Output, Error>> + Send,
            {
                async fn invoke<'a>(&self, _state: RpcContext, input: RawParams<'a>) -> RpcResult {
                    if !input.is_empty() {
                        return Err(RpcError::InvalidParams);
                    }
                    let output = (self.f)().await.map_err(Into::into)?;
                    serde_json::to_value(output).map_err(|e| RpcError::InternalError(e.into()))
                }
            }

            Box::new(Helper {
                f: self,
                _marker: Default::default(),
            })
        }
    }

    /// ```
    /// fn example() -> &'static str
    /// ```
    #[async_trait]
    impl<'a, F> Sealed<(), (), ((), (), &'static str)> for F
    where
        F: Fn() -> &'static str + Sync + Send + 'static,
    {
        fn into_method(self) -> Box<dyn RpcMethod> {
            struct Helper<F> {
                f: F,
            }

            #[axum::async_trait]
            impl<F> RpcMethod for Helper<F>
            where
                F: Fn() -> &'static str + Sync + Send,
            {
                async fn invoke<'a>(&self, _state: RpcContext, input: RawParams<'a>) -> RpcResult {
                    if !input.is_empty() {
                        return Err(RpcError::InvalidParams);
                    }
                    let output = (self.f)();
                    serde_json::to_value(output).map_err(|e| RpcError::InternalError(e.into()))
                }
            }
            Box::new(Helper { f: self })
        }
    }
}

/// Handles invoking an RPC route's methods.
#[async_trait]
pub trait RpcMethodHandler {
    async fn call_method(method: &str, state: RpcContext, params: Value) -> RpcResult;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    async fn spawn_server(router: RpcRouter) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}", addr.port());

        tokio::spawn(async {
            let router = axum::Router::new()
                .route("/", axum::routing::post(rpc_handler))
                .with_state(router);
            axum::Server::from_tcp(listener)
                .unwrap()
                .serve(router.into_make_service())
                .await
        });

        url
    }

    /// Spawns an RPC server with the given router and queries it with the given request.
    async fn serve_and_query(router: RpcRouter, request: Value) -> Value {
        let url = spawn_server(router).await;

        let client = reqwest::Client::new();
        client
            .post(url.clone())
            .json(&request)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    }

    mod specification_tests {
        //! Test cases lifted directly from the [RPC specification](https://www.jsonrpc.org/specification).
        use super::*;
        use pretty_assertions::assert_eq;
        use rstest::rstest;
        use serde_json::json;

        fn spec_router() -> RpcRouter {
            crate::error::generate_rpc_error_subset!(ExampleError:);

            #[derive(Debug, Deserialize, Serialize)]
            struct SubtractInput {
                minuend: i32,
                subtrahend: i32,
            }
            async fn subtract(input: SubtractInput) -> Result<Value, ExampleError> {
                Ok(Value::Number((input.minuend - input.subtrahend).into()))
            }

            #[derive(Debug, Deserialize, Serialize)]
            struct SumInput(Vec<i32>);
            async fn sum(input: SumInput) -> Result<Value, ExampleError> {
                Ok(Value::Number((input.0.iter().sum::<i32>()).into()))
            }

            #[derive(Debug, Deserialize, Serialize)]
            struct GetDataInput;
            #[derive(Debug, Deserialize, Serialize)]
            struct GetDataOutput(Vec<Value>);
            async fn get_data() -> Result<GetDataOutput, ExampleError> {
                Ok(GetDataOutput(vec![
                    Value::String("hello".to_owned()),
                    Value::Number(5.into()),
                ]))
            }

            RpcRouter::builder("vTEST")
                .register("subtract", subtract)
                .register("sum", sum)
                .register("get_data", get_data)
                .build(RpcContext::for_tests())
        }

        #[rstest]
        #[case::with_positional_params(
            json!({"jsonrpc": "2.0", "method": "subtract", "params": [42, 23], "id": 2}),
            json!({"jsonrpc": "2.0", "result": 19, "id": 2}),
        )]
        #[case::with_positional_params_switched(
            json!({"jsonrpc": "2.0", "method": "subtract", "params": [23, 42], "id": 2}),
            json!({"jsonrpc": "2.0", "result": -19, "id": 2}),
        )]
        #[case::with_named_params(
            json!({"jsonrpc": "2.0", "method": "subtract", "params": {"subtrahend": 23, "minuend": 42}, "id": 3}),
            json!({"jsonrpc": "2.0", "result": 19, "id": 3}),
        )]
        #[case::with_named_params_switched(
            json!({"jsonrpc": "2.0", "method": "subtract", "params": {"minuend": 42, "subtrahend": 23}, "id": 4}),
            json!({"jsonrpc": "2.0", "result": 19, "id": 4}),
        )]
        #[case::non_existent_method(
            json!({"jsonrpc": "2.0", "method": "foobar", "id": "1"}),
            json!({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": "1"}),
        )]
        #[case::invalid_request_object(
            json!({"jsonrpc": "2.0", "method": 1, "params": "bar"}),
            json!({"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null}),
        )]
        #[case::empty_batch(
            json!([]),
            json!({"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null}),
        )]
        #[case::invalid_batch_single(
            json!([1]),
            json!([{"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null}]),
        )]
        #[case::invalid_batch_multiple(
            json!([1, 2, 3]),
            json!([
                {"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null},
                {"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null},
                {"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null}
            ]),
        )]
        #[case::batch(
            json!([
                {"jsonrpc": "2.0", "method": "sum", "params": [1,2,4], "id": "1"},
                {"jsonrpc": "2.0", "method": "notify_hello", "params": [7]},
                {"jsonrpc": "2.0", "method": "subtract", "params": [42,23], "id": "2"},
                {"foo": "boo"},
                {"jsonrpc": "2.0", "method": "foo.get", "params": {"name": "myself"}, "id": "5"},
                {"jsonrpc": "2.0", "method": "get_data", "id": "9"}
            ]),
            json!([
                {"jsonrpc": "2.0", "result": 7, "id": "1"},
                {"jsonrpc": "2.0", "result": 19, "id": "2"},
                {"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null},
                {"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": "5"},
                {"jsonrpc": "2.0", "result": ["hello", 5], "id": "9"}
            ]),
        )]
        #[tokio::test]
        async fn specification_test(#[case] request: Value, #[case] expected: Value) {
            let response = serve_and_query(spec_router(), request).await;

            assert_eq!(response, expected);
        }

        #[rstest]
        #[case::with_params(json!({"jsonrpc": "2.0", "method": "update", "params": [1,2,3,4,5]}))]
        #[case::without_params(json!({"jsonrpc": "2.0", "method": "foobar"}))]
        #[case::batch(json!([
            {"jsonrpc": "2.0", "method": "notify_sum", "params": [1,2,4]},
            {"jsonrpc": "2.0", "method": "notify_hello", "params": [7]}
        ]))]
        #[tokio::test]
        async fn notifications(#[case] request: Value) {
            let url = spawn_server(spec_router()).await;

            let client = reqwest::Client::new();
            let res = client
                .post(url.clone())
                .json(&request)
                .send()
                .await
                .unwrap();

            assert_eq!(res.content_length(), Some(0));
        }

        #[rstest]
        #[case::single(r#"{"jsonrpc": "2.0", "method": "foobar, "params": "bar", "baz]"#)]
        #[case::batch(
            r#"[
            {"jsonrpc": "2.0", "method": "sum", "params": [1,2,4], "id": "1"},
            {"jsonrpc": "2.0", "method"
         ]"#
        )]
        #[tokio::test]
        async fn invalid_json(#[case] request: &'static str) {
            let url = spawn_server(spec_router()).await;

            let client = reqwest::Client::new();
            let res = client
                .post(url.clone())
                .body(request)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap();

            let expected = serde_json::json!({"jsonrpc": "2.0", "error": {"code": -32700, "message": "Parse error"}, "id": null});
            assert_eq!(res, expected);
        }
    }

    mod panic_handling {
        use super::*;

        fn panic_router() -> RpcRouter {
            fn always_panic() -> &'static str {
                panic!("Oh no!");
            }

            fn always_success() -> &'static str {
                "Success"
            }

            RpcRouter::builder("vTest")
                .register("panic", always_panic)
                .register("success", always_success)
                .build(RpcContext::for_tests())
        }

        #[tokio::test]
        async fn panic_is_internal_error() {
            let response = serve_and_query(
                panic_router(),
                json!(
                    {"jsonrpc": "2.0", "method": "panic", "id": 1}
                ),
            )
            .await;

            let expected = serde_json::json!({"jsonrpc": "2.0", "error": {"code": -32603, "message": "Internal error"}, "id": 1});
            assert_eq!(response, expected);
        }

        #[tokio::test]
        async fn panic_in_batch_is_isolated() {
            let response = serve_and_query(
                panic_router(),
                json!([
                    {"jsonrpc": "2.0", "method": "panic", "id": 1},
                    {"jsonrpc": "2.0", "method": "success", "id": 2},
                ]),
            )
            .await;

            let expected = serde_json::json!([
                {"jsonrpc": "2.0", "error": {"code": -32603, "message": "Internal error"}, "id": 1},
                {"jsonrpc": "2.0", "result": "Success", "id": 2},
            ]);
            assert_eq!(response, expected);
        }
    }

    #[tokio::test]
    async fn rejects_non_json_content_header() {
        async fn always_success(_ctx: RpcContext) -> RpcResult {
            Ok(json!("Success"))
        }

        let router = RpcRouter::builder("vTEST")
            .register("success", always_success)
            .build(RpcContext::for_tests());

        let url = spawn_server(router).await;

        let client = reqwest::Client::new();
        let res = client
            .post(url.clone())
            .body(
                json!(
                    {"jsonrpc": "2.0", "method": "any", "id": 1}
                )
                .to_string(),
            )
            .header(reqwest::header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .send()
            .await
            .unwrap()
            .status();

        assert_eq!(res, reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn with_no_params() {
        fn always_success() -> &'static str {
            "Success"
        }

        let router = RpcRouter::builder("vTEST")
            .register("success", always_success)
            .build(RpcContext::for_tests());

        let url = spawn_server(router).await;

        let expected = json!({"jsonrpc": "2.0", "result": "Success", "id": 1});

        let client = reqwest::Client::new();
        let res = client
            .post(url.clone())
            .json(&json!(
                {"jsonrpc": "2.0", "method": "success", "id": 1}
            ))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();

        assert_eq!(res, expected);

        let res = client
            .post(url.clone())
            .json(&json!(
                {"jsonrpc": "2.0", "method": "success", "id": 1, "params": []}
            ))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();

        assert_eq!(res, expected);
    }
}
