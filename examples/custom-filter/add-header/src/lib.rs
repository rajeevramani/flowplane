use proxy_wasm::traits::*;
use proxy_wasm::types::*;

proxy_wasm::main! {{
    proxy_wasm::set_log_level(LogLevel::Trace);
    proxy_wasm::set_root_context(|_| -> Box<dyn RootContext> {
        Box::new(AddHeaderRoot)
    });
}}

struct AddHeaderRoot;

impl Context for AddHeaderRoot {}

impl RootContext for AddHeaderRoot {
    fn get_type(&self) -> Option<ContextType> {
        Some(ContextType::HttpContext)
    }

    fn create_http_context(&self, _context_id: u32) -> Option<Box<dyn HttpContext>> {
        Some(Box::new(AddHeaderFilter))
    }
}

struct AddHeaderFilter;

impl Context for AddHeaderFilter {}

impl HttpContext for AddHeaderFilter {
    fn on_http_request_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        // Add a header to the request going upstream
        self.add_http_request_header("x-wasm-filter", "processed");

        log::info!("WASM filter: Added x-wasm-filter header to request");

        Action::Continue
    }

    fn on_http_response_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        // Add a header to the response going to client
        self.add_http_response_header("x-wasm-response", "added");

        log::info!("WASM filter: Added x-wasm-response header to response");

        Action::Continue
    }
}
