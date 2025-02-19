use std::borrow::Cow;

use serde::Serialize;

#[derive(Debug)]
pub enum RpcError {
    ParseError,
    InvalidRequest,
    MethodNotFound,
    InvalidParams,
    InternalError(anyhow::Error),
    ApplicationError { code: i32, message: String },
}

impl PartialEq for RpcError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::InternalError(l0), Self::InternalError(r0)) => l0.to_string() == r0.to_string(),
            (
                Self::ApplicationError {
                    code: l_code,
                    message: l_message,
                },
                Self::ApplicationError {
                    code: r_code,
                    message: r_message,
                },
            ) => l_code == r_code && l_message == r_message,
            _ => core::mem::discriminant(self) == core::mem::discriminant(other),
        }
    }
}

impl RpcError {
    pub fn code(&self) -> i32 {
        // From the json-rpc specification: https://www.jsonrpc.org/specification#error_object
        match self {
            RpcError::ParseError => -32700,
            RpcError::InvalidRequest => -32600,
            RpcError::MethodNotFound { .. } => -32601,
            RpcError::InvalidParams => -32602,
            RpcError::InternalError(_) => -32603,
            RpcError::ApplicationError { code, .. } => *code,
        }
    }

    pub fn message(&self) -> Cow<'_, str> {
        match self {
            RpcError::ParseError => "Parse error".into(),
            RpcError::InvalidRequest => "Invalid Request".into(),
            RpcError::MethodNotFound { .. } => "Method not found".into(),
            RpcError::InvalidParams => "Invalid params".into(),
            // TODO: this is not necessarily a good idea. All internal errors are returned here, even
            // ones that we probably should not disclose.
            RpcError::InternalError(e) => e.to_string().into(),
            RpcError::ApplicationError { message, .. } => message.into(),
        }
    }
}

impl Serialize for RpcError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        let mut obj = serializer.serialize_map(Some(2))?;
        obj.serialize_entry("code", &self.code())?;
        obj.serialize_entry("message", &self.message())?;
        obj.end()
    }
}

impl<E> From<E> for RpcError
where
    E: Into<crate::error::RpcError>,
{
    fn from(value: E) -> Self {
        match value.into() {
            crate::error::RpcError::GatewayError(x) => RpcError::InternalError(x.into()),
            crate::error::RpcError::Internal(x) => RpcError::InternalError(x),
            other => RpcError::ApplicationError {
                code: other.code(),
                message: format!("{other}"),
            },
        }
    }
}
