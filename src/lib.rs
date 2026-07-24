use act_sdk::prelude::*;

pub mod addr;
pub mod demux;
pub mod logbuf;
pub mod proto;

#[act_component]
mod component {
    use super::*;

    /// Say hello.
    #[act_tool(description = "Say hello", read_only)]
    fn hello(
        /// Name to greet
        name: Option<String>,
    ) -> ActResult<String> {
        let who = name.unwrap_or_else(|| "world".to_string());
        Ok(format!("Hello, {who}!"))
    }
}