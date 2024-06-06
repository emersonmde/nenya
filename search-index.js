var searchIndex = new Map(JSON.parse('[\
["nenya",{"doc":"Distributed Rate Limiting System","t":"FFNNNNNNNNNNNNNNNNNNNCNNNNNNNNNNNNNNFFNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNN","n":["RateLimiter","RateLimiterBuilder","accepted_request_rate","borrow","borrow","borrow_mut","borrow_mut","build","external_accepted_request_rate","external_accepted_request_rate","external_request_rate","external_request_rate","fmt","from","from","into","into","max_rate","min_rate","new","new","pid_controller","pid_controller","request_rate","set_external_accepted_request_rate","set_external_request_rate","setpoint","should_throttle","target_rate","try_from","try_from","try_into","try_into","type_id","type_id","update_interval","PIDController","PIDControllerBuilder","accumulated_error","borrow","borrow","borrow_mut","borrow_mut","build","clone","clone_into","compute_correction","error_bias","error_limit","fmt","from","from","into","into","kd","ki","kp","new","new","new_static_controller","output_limit","setpoint","to_owned","try_from","try_from","try_into","try_into","type_id","type_id"],"q":[[0,"nenya"],[36,"nenya::pid_controller"],[69,"num_traits::float"],[70,"num_traits::sign"],[71,"num_traits::cast"],[72,"core::marker"],[73,"core::fmt"],[74,"core::fmt"],[75,"core::convert"],[76,"core::result"],[77,"core::any"],[78,"core::clone"],[79,"core::option"]],"d":["Sliding window rate limiter with an integrated PID …","Builder for creating a <code>RateLimiter</code> instance.","Returns the current accepted request rate.","","","","","Builds and returns the <code>RateLimiter</code> instance.","Sets the external accepted request rate.","Returns the current external accepted request rate.","Sets the external request rate.","Returns the current external request rate.","","Returns the argument unchanged.","Returns the argument unchanged.","Calls <code>U::from(self)</code>.","Calls <code>U::from(self)</code>.","Sets the maximum allowable rate of requests.","Sets the minimum allowable rate of requests.","Creates a new <code>RateLimiterBuilder</code> with default values.","Creates a new <code>RateLimiter</code> instance.","","Sets the PID controller for the rate limiter.","Returns the current request rate.","Sets the external accepted request rate.","Sets the external request rate.","Returns the current setpoint of the PID controller.","Determines if the current request should be throttled …","Returns the current target rate of the rate limiter.","","","","","","","Sets the update interval for the PID controller.","","Builder for creating a <code>PIDController</code> instance.","Returns the accumulated error of the PID controller.","","","","","Builds and returns the <code>PIDController</code> instance.","","","Computes the correction based on the current error.","Sets the error bias.","Sets the error limit.","","Returns the argument unchanged.","Returns the argument unchanged.","Calls <code>U::from(self)</code>.","Calls <code>U::from(self)</code>.","Sets the derivative gain (<code>kd</code>).","Sets the integral gain (<code>ki</code>).","Sets the proportional gain (<code>kp</code>).","Creates a new <code>PIDControllerBuilder</code> with default values.","Creates a new <code>PIDController</code>.","Creates a new static <code>PIDController</code> with zero gains.","Sets the output limit.","Returns the setpoint of the PID controller.","","","","","","",""],"i":[0,0,1,6,1,6,1,6,6,1,6,1,1,6,1,6,1,6,6,6,1,0,6,1,1,1,1,1,1,6,1,6,1,6,1,6,0,0,10,17,10,17,10,17,10,10,10,17,17,10,17,10,17,10,17,17,17,17,10,10,17,10,10,17,10,17,10,17,10],"f":"``{{{b{c}}}c{dfhj}}{ce{}{}}000{{{l{c}}}{{b{c}}}{dfhj}}{{{l{c}}c}{{l{c}}}{dfhj}}303{{{b{c}}n}A`Ab}{cc{}}04422{c{{l{c}}}{dfhj}}{{ccc{Ad{c}}Af}{{b{c}}}{dfhj}}`{{{l{c}}{Ad{c}}}{{l{c}}}{dfhj}}8{{{b{c}}e}Ah{dfhj}{{Aj{c}}}}09{{{b{c}}}Al{dfhj}}:{c{{An{e}}}{}{}}000{cB`{}}0{{{l{c}}Af}{{l{c}}}{dfhj}}``{{{Ad{c}}}c{dfj}}===={{{Bb{c}}}{{Ad{c}}}{dfj}}{{{Ad{c}}}{{Ad{c}}}Bd}{{ce}Ah{}{}}{{{Ad{c}}e}c{dfj}{{Aj{c}}}}{{{Bb{c}}e}{{Bb{c}}}{dfj}{{Aj{c}}}}0{{{Ad{c}}n}A`Ab}??{ce{}{}}0222{e{{Bb{c}}}{dfj}{{Aj{c}}}}{{ccccc{Bf{c}}{Bf{c}}}{{Ad{c}}}{dfj}}{c{{Ad{c}}}{dfj}}5:3====<<","c":[],"p":[[5,"RateLimiter",0],[10,"Float",69],[10,"Signed",70],[10,"FromPrimitive",71],[10,"Copy",72],[5,"RateLimiterBuilder",0],[5,"Formatter",73],[8,"Result",73],[10,"Debug",73],[5,"PIDController",36],[5,"Duration",74],[1,"unit"],[10,"Into",75],[1,"bool"],[6,"Result",76],[5,"TypeId",77],[5,"PIDControllerBuilder",36],[10,"Clone",78],[6,"Option",79]],"b":[]}],\
["nenya_sentinel",{"doc":"","t":"IIFOOONNNNNNONNHNOOOOCNNNNNFFFFFONNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNONOONOOCCOOONNNNNNNNNNNNNNNNNNNNNNNNNFNNNNNNNNNNONNNNNNNNNNNNNNKFFNONNNNNNNNNNMNNNNNNNONNNNNNONONNNNOMNNNNNNNNNNN","n":["LockedSegmentMetrics","SegmentMetrics","SentinelService","_default_segment_config","alloc","base","borrow","borrow_mut","default","exchange_metrics","fmt","from","hostname","into","into_request","main","new","node_metrics","phantom","ptr","segments","sentinel","should_throttle","try_from","try_into","type_id","vzip","MetricData","Metrics","SegmentConfig","ShouldThrottleRequest","ShouldThrottleResponse","accepted_request_rate","borrow","borrow","borrow","borrow","borrow","borrow_mut","borrow_mut","borrow_mut","borrow_mut","borrow_mut","clear","clear","clear","clear","clear","clone","clone","clone","clone","clone","clone_into","clone_into","clone_into","clone_into","clone_into","default","default","default","default","default","encoded_len","encoded_len","encoded_len","encoded_len","encoded_len","eq","eq","eq","eq","eq","fmt","fmt","fmt","fmt","fmt","from","from","from","from","from","from_ref","from_ref","from_ref","from_ref","from_ref","into","into","into","into","into","into_request","into_request","into_request","into_request","into_request","max_tps","max_tps","min_tps","min_tps","request_rate","segment","segment","segments","sentinel_client","sentinel_server","should_throttle","source","target_tps","to_owned","to_owned","to_owned","to_owned","to_owned","try_from","try_from","try_from","try_from","try_from","try_into","try_into","try_into","try_into","try_into","type_id","type_id","type_id","type_id","type_id","vzip","vzip","vzip","vzip","vzip","SentinelClient","accept_compressed","borrow","borrow_mut","clone","clone_into","connect","exchange_metrics","fmt","from","from_ref","inner","into","into_request","max_decoding_message_size","max_encoding_message_size","new","send_compressed","should_throttle","to_owned","try_from","try_into","type_id","vzip","with_interceptor","with_origin","Sentinel","SentinelServer","_Inner","accept_compressed","accept_compression_encodings","borrow","borrow","borrow_mut","borrow_mut","call","call","clone","clone","clone_into","clone_into","exchange_metrics","fmt","fmt","from","from","from_arc","from_ref","from_ref","inner","into","into","into_make_service","into_request","into_request","max_decoding_message_size","max_decoding_message_size","max_encoding_message_size","max_encoding_message_size","new","poll_ready","poll_ready","send_compressed","send_compression_encodings","should_throttle","to_owned","to_owned","try_from","try_from","try_into","try_into","type_id","type_id","vzip","vzip","with_interceptor"],"q":[[0,"nenya_sentinel"],[27,"nenya_sentinel::sentinel"],[136,"nenya_sentinel::sentinel::sentinel_client"],[162,"nenya_sentinel::sentinel::sentinel_server"],[212,"tonic::request"],[213,"core::future::future"],[214,"alloc::boxed"],[215,"core::pin"],[216,"core::fmt"],[217,"core::fmt"],[218,"core::result"],[219,"alloc::string"],[220,"alloc::vec"],[221,"std::collections::hash::map"],[222,"nenya::pid_controller"],[223,"core::any"],[224,"tonic::codec::compression"],[225,"tonic::body"],[226,"tonic::client::service"],[227,"core::clone"],[228,"tonic::transport::channel"],[229,"tonic::transport::error"],[230,"tonic::transport::channel::endpoint"],[231,"core::convert"],[232,"tonic::response"],[233,"tonic::status"],[234,"tonic::request"],[235,"http::request"],[236,"http::response"],[237,"tower_service"],[238,"tonic::service::interceptor"],[239,"http_body"],[240,"core::marker"],[241,"alloc::sync"],[242,"axum::routing::into_make_service"],[243,"core::task::wake"],[244,"core::task::poll"]],"d":["","","","","","","","","","","","Returns the argument unchanged.","","Calls <code>U::from(self)</code>.","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","Returns the argument unchanged.","Returns the argument unchanged.","Returns the argument unchanged.","Returns the argument unchanged.","Returns the argument unchanged.","","","","","","Calls <code>U::from(self)</code>.","Calls <code>U::from(self)</code>.","Calls <code>U::from(self)</code>.","Calls <code>U::from(self)</code>.","Calls <code>U::from(self)</code>.","","","","","","Returns the value of <code>max_tps</code>, or the default value if …","","Returns the value of <code>min_tps</code>, or the default value if …","","","Returns the value of <code>segment</code>, or the default value if …","","","Generated client implementations.","Generated server implementations.","","","","","","","","","","","","","","","","","","","","","","","","","","","","","","Enable decompressing responses.","","","","","Attempt to create a new client by connecting to a given …","","","Returns the argument unchanged.","","","Calls <code>U::from(self)</code>.","","Limits the maximum size of a decoded message.","Limits the maximum size of an encoded message.","","Compress requests with the given encoding.","","","","","","","","","Generated trait containing gRPC methods that should be …","","","Enable decompressing requests with the given encoding.","","","","","","","","","","","","","","","Returns the argument unchanged.","Returns the argument unchanged.","","","","","Calls <code>U::from(self)</code>.","Calls <code>U::from(self)</code>.","","","","Limits the maximum size of a decoded message.","","Limits the maximum size of an encoded message.","","","","","Compress responses with the given encoding, if the client …","","","","","","","","","","","","",""],"i":[0,0,0,1,54,55,1,1,1,1,1,1,1,1,1,0,1,1,54,54,1,0,1,1,1,1,1,0,0,0,0,0,20,2,20,18,21,14,2,20,18,21,14,2,20,18,21,14,2,20,18,21,14,2,20,18,21,14,2,20,18,21,14,2,20,18,21,14,2,20,18,21,14,2,20,18,21,14,2,20,18,21,14,2,20,18,21,14,2,20,18,21,14,2,20,18,21,14,14,14,14,14,20,18,18,2,0,0,21,2,14,2,20,18,21,14,2,20,18,21,14,2,20,18,21,14,2,20,18,21,14,2,20,18,21,14,0,25,25,25,25,25,25,25,25,25,25,25,25,25,25,25,25,25,25,25,25,25,25,25,25,25,0,0,0,45,45,49,45,49,45,45,45,49,45,49,45,46,49,45,49,45,45,49,45,45,49,45,45,49,45,45,45,45,45,45,45,45,45,45,46,49,45,49,45,49,45,49,45,49,45,45],"f":"``````{ce{}{}}0{{}b}{{b{f{d}}}{{l{{j{h}}}}}}{{bn}A`}{cc{}}`4{c{{f{e}}}{}{}}{{}{{Af{Ab{j{Ad}}}}}}{{Ah{Aj{Ah}}{An{AhAl}}Al{Bb{B`}}}b}`````{{b{f{Bd}}}{{l{{j{h}}}}}}{c{{Af{e}}}{}{}}0{cBf{}}:``````::::::::::{dAb}{BhAb}{BdAb}{BjAb}{AlAb}{dd}{BhBh}{BdBd}{BjBj}{AlAl}{{ce}Ab{}{}}0000{{}d}{{}Bh}{{}Bd}{{}Bj}{{}Al}{dBl}{BhBl}{BdBl}{BjBl}{AlBl}{{dd}Bn}{{BhBh}Bn}{{BdBd}Bn}{{BjBj}Bn}{{AlAl}Bn}{{dn}A`}{{Bhn}A`}{{Bdn}A`}{{Bjn}A`}{{Aln}A`}{cc{}}000000000{ce{}{}}0000{c{{f{e}}}{}{}}0000{AlB`}`0``{BdC`}```````33333{c{{Af{e}}}{}{}}000000000{cBf{}}000055555`{{{Cb{c}}Cd}{{Cb{c}}}{{Ch{Cf}}}}66{{{Cb{c}}}{{Cb{c}}}Cj}{{ce}Ab{}{}}{c{{Af{{Cb{Cl}}Cn}}}{{Db{D`}}}}{{{Cb{c}}e}{{Af{{Dd{d}}Df}}}{{Ch{Cf}}}{{Dh{d}}}}{{{Cb{c}}n}A`Dj}<<`;:{{{Cb{c}}Bl}{{Cb{c}}}{{Ch{Cf}}}}0{c{{Cb{c}}}{{Ch{Cf}}}}7{{{Cb{c}}e}{{Af{{Dd{Bj}}Df}}}{{Ch{Cf}}}{{Dh{Bd}}}}>::9>{{ce}{{Cb{{Dl{ce}}}}}{{Ed{{Dn{Cf}}}{{E`{Eb}}}}{Ch{Cf}}}Ef}{{cEh}{{Cb{c}}}{{Ch{Cf}}}}```{{{Ej{c}}Cd}{{Ej{c}}}El}`{ce{}{}}000{{{Ej{c}}{Dn{e}}}gEl{EnF`}{}}{{c{Dn{e}}}{}{}{}}{{{Fb{c}}}{{Fb{c}}}El}{{{Ej{c}}}{{Ej{c}}}El}>>{{El{f{d}}}{{l{{j{h}}}}}}{{{Fb{c}}n}A`Dj}{{{Ej{c}}n}A`{DjEl}}{cc{}}0{{{Fd{c}}}{{Ej{c}}}El}11`99{c{{Ff{e}}}{}{}}{c{{f{e}}}{}{}}0{{{Ej{c}}Bl}{{Ej{c}}}El}`0`{c{{Ej{c}}}El}{{cFh}{{Fj{{Af{Ab}}}}}{}}{{{Ej{c}}Fh}{{Fj{{Af{Abe}}}}}El{}}{{{Ej{c}}Cd}{{Ej{c}}}El}`{{El{f{Bd}}}{{l{{j{h}}}}}}{ce{}{}}0{c{{Af{e}}}{}{}}000{cBf{}}022{{ce}{{Dl{{Ej{c}}e}}}ElEf}","c":[],"p":[[5,"SentinelService",0],[5,"Metrics",27],[5,"Request",212],[10,"Future",213],[5,"Box",214],[5,"Pin",215],[5,"Formatter",216],[8,"Result",216],[1,"unit"],[10,"Error",217],[6,"Result",218],[5,"String",219],[5,"Vec",220],[5,"SegmentConfig",27],[5,"HashMap",221],[1,"f32"],[5,"PIDController",222],[5,"ShouldThrottleRequest",27],[5,"TypeId",223],[5,"MetricData",27],[5,"ShouldThrottleResponse",27],[1,"usize"],[1,"bool"],[1,"str"],[5,"SentinelClient",136],[6,"CompressionEncoding",224],[8,"BoxBody",225],[10,"GrpcService",226],[10,"Clone",227],[5,"Channel",228],[5,"Error",229],[5,"Endpoint",230],[10,"TryInto",231],[5,"Response",232],[5,"Status",233],[10,"IntoRequest",212],[10,"Debug",216],[5,"InterceptedService",234],[5,"Request",235],[17,"Response"],[5,"Response",236],[10,"Service",237],[10,"Interceptor",234],[5,"Uri",238],[5,"SentinelServer",162],[10,"Sentinel",162],[10,"Body",239],[10,"Send",240],[5,"_Inner",162],[5,"Arc",241],[5,"IntoMakeService",242],[5,"Context",243],[6,"Poll",244],[8,"LockedSegmentMetrics",0],[8,"SegmentMetrics",0]],"b":[]}]\
]'));
if (typeof exports !== 'undefined') exports.searchIndex = searchIndex;
else if (window.initSearch) window.initSearch(searchIndex);
