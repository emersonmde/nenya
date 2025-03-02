(function() {
    var implementors = Object.fromEntries([["nenya_sentinel",[["impl&lt;T, B&gt; Service&lt;<a class=\"struct\" href=\"https://docs.rs/http/0.2.11/http/request/struct.Request.html\" title=\"struct http::request::Request\">Request</a>&lt;B&gt;&gt; for <a class=\"struct\" href=\"nenya_sentinel/sentinel/sentinel_server/struct.SentinelServer.html\" title=\"struct nenya_sentinel::sentinel::sentinel_server::SentinelServer\">SentinelServer</a>&lt;T&gt;<div class=\"where\">where\n    T: <a class=\"trait\" href=\"nenya_sentinel/sentinel/sentinel_server/trait.Sentinel.html\" title=\"trait nenya_sentinel::sentinel::sentinel_server::Sentinel\">Sentinel</a>,\n    B: <a class=\"trait\" href=\"https://docs.rs/http-body/0.4.6/http_body/trait.Body.html\" title=\"trait http_body::Body\">Body</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.85.0/core/marker/trait.Send.html\" title=\"trait core::marker::Send\">Send</a> + 'static,\n    B::<a class=\"associatedtype\" href=\"https://docs.rs/http-body/0.4.6/http_body/trait.Body.html#associatedtype.Error\" title=\"type http_body::Body::Error\">Error</a>: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.85.0/core/convert/trait.Into.html\" title=\"trait core::convert::Into\">Into</a>&lt;<a class=\"type\" href=\"https://docs.rs/tonic/0.11.0/tonic/codegen/type.StdError.html\" title=\"type tonic::codegen::StdError\">StdError</a>&gt; + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.85.0/core/marker/trait.Send.html\" title=\"trait core::marker::Send\">Send</a> + 'static,</div>"]]]]);
    if (window.register_implementors) {
        window.register_implementors(implementors);
    } else {
        window.pending_implementors = implementors;
    }
})()
//{"start":57,"fragment_lengths":[1504]}