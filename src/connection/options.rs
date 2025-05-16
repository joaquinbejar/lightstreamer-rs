use crate::client::Transport;
use crate::utils::{IllegalArgumentException, Proxy};
use std::collections::HashMap;
use std::fmt::{self, Debug, Formatter};

/// Used by LightstreamerClient to provide an extra connection properties data object.
/// Data struct that contains the policy settings used to connect to a Lightstreamer Server.
/// An instance of this struct is attached to every LightstreamerClient as connection_options.
///
/// See also `LightstreamerClient`
pub struct ConnectionOptions {
    content_length: Option<u64>,
    first_retry_max_delay: u64,
    forced_transport: Option<Transport>,
    http_extra_headers: Option<HashMap<String, String>>,
    http_extra_headers_on_session_creation_only: bool,
    idle_timeout: u64,
    keepalive_interval: u64,
    polling_interval: u64,
    proxy: Option<Proxy>,
    real_max_bandwidth: Option<u64>,
    reconnect_timeout: u64,
    requested_max_bandwidth: Option<f64>,
    retry_delay: u64,
    reverse_heartbeat_interval: u64,
    server_instance_address_ignored: bool,
    session_recovery_timeout: u64,
    slowing_enabled: bool,
    stalled_timeout: u64,
    send_sync: bool,
    _reduce_head: bool,
    supported_diffs: Option<String>,
    polling: bool,
    ttl_millis: Option<u64>,
}

impl ConnectionOptions {
    /// Creates a new instance of `ConnectionOptions` with default values.
    pub fn new() -> Self {
        ConnectionOptions {
            content_length: None,
            first_retry_max_delay: 100,
            forced_transport: None,
            http_extra_headers: None,
            http_extra_headers_on_session_creation_only: false,
            idle_timeout: 19000,
            keepalive_interval: 0,
            polling_interval: 0,
            proxy: None,
            real_max_bandwidth: None,
            reconnect_timeout: 3000,
            requested_max_bandwidth: None,
            retry_delay: 4000,
            reverse_heartbeat_interval: 0,
            session_recovery_timeout: 15000,
            slowing_enabled: false,
            stalled_timeout: 2000,
            server_instance_address_ignored: false,
            send_sync: true,
            _reduce_head: false,
            supported_diffs: None,
            polling: false,
            ttl_millis: None,
        }
    }

    /// Inquiry method that gets the length expressed in bytes to be used by the Server for the
    /// response body on a HTTP stream connection.
    ///
    /// # Returns
    ///
    /// The length to be used by the Server for the response body on a HTTP stream connection
    ///
    /// See also `setContentLength()`
    pub fn get_content_length(&self) -> Option<u64> {
        self.content_length
    }

    /// Inquiry method that gets the maximum time to wait before trying a new connection to the
    /// Server in case the previous one is unexpectedly closed while correctly working.
    ///
    /// # Returns
    ///
    /// The max time (in milliseconds) to wait before trying a new connection.
    ///
    /// See also `setFirstRetryMaxDelay()`
    pub fn get_first_retry_max_delay(&self) -> u64 {
        self.first_retry_max_delay
    }

    /// Inquiry method that gets the value of the forced transport (if any).
    ///
    /// # Returns
    ///
    /// The forced transport or `None`
    ///
    /// See also `setForcedTransport()`
    pub fn get_forced_transport(&self) -> Option<&Transport> {
        self.forced_transport.as_ref()
    }

    /// Inquiry method that gets the Map object containing the extra headers to be sent to the server.
    ///
    /// # Returns
    ///
    /// The Map object containing the extra headers to be sent
    ///
    /// See also `setHttpExtraHeaders()`
    ///
    /// See also `setHttpExtraHeadersOnSessionCreationOnly()`
    pub fn get_http_extra_headers(&self) -> Option<&HashMap<String, String>> {
        self.http_extra_headers.as_ref()
    }

    /// Inquiry method that gets the maximum time the Server is allowed to wait for any data to be
    /// sent in response to a polling request, if none has accumulated at request time. The wait
    /// time used by the Server, however, may be different, because of server side restrictions.
    ///
    /// # Returns
    ///
    /// The time (in milliseconds) the Server is allowed to wait for data to send upon polling requests.
    ///
    /// See also `setIdleTimeout()`
    pub fn get_idle_timeout(&self) -> u64 {
        self.idle_timeout
    }

    /// Inquiry method that gets the interval between two keepalive packets sent by Lightstreamer
    /// Server on a stream connection when no actual data is being transmitted. If the returned
    /// value is 0, it means that the interval is to be decided by the Server upon the next connection.
    ///
    /// If the value has just been set and a connection to Lightstreamer Server has not been established
    /// yet, the returned value is the time that is being requested to the Server. Afterwards, the
    /// returned value is the time used by the Server, that may be different, because of Server side
    /// constraints.
    ///
    /// # Returns
    ///
    /// The time, expressed in milliseconds, between two keepalive packets sent by the Server, or 0.
    ///
    /// See also `setKeepaliveInterval()`
    pub fn get_keepalive_interval(&self) -> u64 {
        self.keepalive_interval
    }

    /// Inquiry method that gets the polling interval used for polling connections.
    ///
    /// If the value has just been set and a polling request to Lightstreamer Server has not been
    /// performed yet, the returned value is the polling interval that is being requested to the
    /// Server. Afterwards, the returned value is the the time between subsequent polling requests
    /// that is really allowed by the Server, that may be different, because of Server side constraints.
    ///
    /// # Returns
    ///
    /// The time (in milliseconds) between subsequent polling requests.
    ///
    /// See also `setPollingInterval()`
    pub fn get_polling_interval(&self) -> u64 {
        self.polling_interval
    }

    /// Inquiry method that gets the maximum bandwidth that can be consumed for the data coming
    /// from Lightstreamer Server. This is the actual maximum bandwidth, in contrast with the requested
    /// maximum bandwidth, returned by `get_requested_max_bandwidth()`.
    ///
    /// The value may differ from the requested one because of restrictions operated on the server
    /// side, or because bandwidth management is not supported (in this case it is always "unlimited"),
    /// but also because of number rounding.
    ///
    /// If a connection to Lightstreamer Server is not currently active, `None` is returned; soon
    /// after the connection is established, the value becomes available, as notified by a call to
    /// `ClientListener.onPropertyChange()` with argument "realMaxBandwidth".
    ///
    /// # Returns
    ///
    /// A decimal number, which represents the maximum bandwidth applied by the Server for the streaming
    /// or polling connection expressed in kbps (kilobits/sec), or the string "unlimited", or `None`.
    ///
    /// See also `setRequestedMaxBandwidth()`
    pub fn get_real_max_bandwidth(&self) -> Option<f64> {
        // Implementation to get the actual maximum bandwidth from the server
        unimplemented!()
    }

    /// Inquiry method that gets the time the client, after entering "STALLED" status, is allowed
    /// to keep waiting for a keepalive packet or any data on a stream connection, before disconnecting
    /// and trying to reconnect to the Server.
    ///
    /// # Returns
    ///
    /// The idle time (in milliseconds) admitted in "STALLED" status before trying to reconnect
    /// to the Server.
    ///
    /// See also `setReconnectTimeout()`
    pub fn get_reconnect_timeout(&self) -> u64 {
        self.reconnect_timeout
    }

    /// Inquiry method that gets the maximum bandwidth that can be consumed for the data coming
    /// from Lightstreamer Server, as requested for this session. The maximum bandwidth limit really
    /// applied by the Server on the session is provided by `get_real_max_bandwidth()`
    ///
    /// # Returns
    ///
    /// A decimal number, which represents the maximum bandwidth requested for the streaming or polling
    /// connection expressed in kbps (kilobits/sec), or the string "unlimited".
    ///
    /// See also `setRequestedMaxBandwidth()`
    pub fn get_requested_max_bandwidth(&self) -> Option<f64> {
        self.requested_max_bandwidth
    }

    /// Inquiry method that gets the minimum time to wait before trying a new connection to the
    /// Server in case the previous one failed for any reason, which is also the maximum time to
    /// wait for a response to a request before dropping the connection and trying with a different
    /// approach. Note that the delay is calculated from the moment the effort to create a connection
    /// is made, not from the moment the failure is detected or the connection timeout expires.
    ///
    /// # Returns
    ///
    /// The time (in milliseconds) to wait before trying a new connection.
    ///
    /// See also `setRetryDelay()`
    pub fn get_retry_delay(&self) -> u64 {
        self.retry_delay
    }

    /// Inquiry method that gets the reverse-heartbeat interval expressed in milliseconds. A 0 value
    /// is possible, meaning that the mechanism is disabled.
    ///
    /// # Returns
    ///
    /// The reverse-heartbeat interval, or 0.
    ///
    /// See also `setReverseHeartbeatInterval()`
    pub fn get_reverse_heartbeat_interval(&self) -> u64 {
        self.reverse_heartbeat_interval
    }

    /// Inquiry method that gets if LS_send_sync is to be sent to the server.
    /// If set to false, instructs the Server not to send the SYNC notifications on this connection.
    /// If omitted, the default is true.
    pub fn get_send_sync(&self) -> bool {
        self.send_sync
    }

    /// Inquiry method that gets the maximum time allowed for attempts to recover the current session
    /// upon an interruption, after which a new session will be created. A 0 value also means that
    /// any attempt to recover the current session is prevented in the first place.
    ///
    /// # Returns
    ///
    /// The maximum time allowed for recovery attempts, possibly 0.
    ///
    /// See also `setSessionRecoveryTimeout()`
    pub fn get_session_recovery_timeout(&self) -> u64 {
        self.session_recovery_timeout
    }

    /// Inquiry method that gets the extra time the client can wait when an expected keepalive packet
    /// has not been received on a stream connection (and no actual data has arrived), before entering
    /// the "STALLED" status.
    ///
    /// # Returns
    ///
    /// The idle time (in milliseconds) admitted before entering the "STALLED" status.
    ///
    /// See also `setStalledTimeout()`
    pub fn get_stalled_timeout(&self) -> u64 {
        self.stalled_timeout
    }

    /// Inquiry method that checks if the restriction on the forwarding of the configured extra
    /// http headers applies or not.
    ///
    /// # Returns
    ///
    /// `true`/`false` if the restriction applies or not.
    ///
    /// See also `setHttpExtraHeadersOnSessionCreationOnly()`
    ///
    /// See also `setHttpExtraHeaders()`
    pub fn is_http_extra_headers_on_session_creation_only(&self) -> bool {
        self.http_extra_headers_on_session_creation_only
    }

    /// Inquiry method that checks if the client is going to ignore the server instance address
    /// that will possibly be sent by the server.
    ///
    /// # Returns
    ///
    /// Whether or not to ignore the server instance address sent by the server.
    ///
    /// See also `setServerInstanceAddressIgnored()`
    pub fn is_server_instance_address_ignored(&self) -> bool {
        self.server_instance_address_ignored
    }

    /// Inquiry method that checks if the slowing algorithm is enabled or not.
    ///
    /// # Returns
    ///
    /// Whether the slowing algorithm is enabled or not.
    ///
    /// See also `setSlowingEnabled()`
    pub fn is_slowing_enabled(&self) -> bool {
        self.slowing_enabled
    }

    /// Setter method that sets the length in bytes to be used by the Server for the response body
    /// on a stream connection (a minimum length, however, is ensured by the server). After the
    /// content length exhaustion, the connection will be closed and a new bind connection will
    /// be automatically reopened.
    ///
    /// NOTE that this setting only applies to the "HTTP-STREAMING" case (i.e. not to WebSockets).
    ///
    /// A length decided by the library, to ensure the best performance. It can be of a few MB
    /// or much higher, depending on the environment.
    ///
    /// The content length should be set before calling the `LightstreamerClient.connect()` method.
    /// However, the value can be changed at any time: the supplied value will be used for the
    /// next streaming connection (either a bind or a brand new session).
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "contentLength" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `content_length`: The length to be used by the Server for the response body on a HTTP
    ///   stream connection.
    ///
    /// # Raises
    ///
    /// * `IllegalArgumentException`: if a negative or zero value is configured
    pub fn set_content_length(
        &mut self,
        content_length: u64,
    ) -> Result<(), IllegalArgumentException> {
        if content_length == 0 {
            return Err(IllegalArgumentException::new(
                "Content length cannot be zero",
            ));
        }

        self.content_length = Some(content_length);
        Ok(())
    }

    /// Setter method that sets the maximum time to wait before trying a new connection to the Server
    /// in case the previous one is unexpectedly closed while correctly working. The new connection
    /// may be either the opening of a new session or an attempt to recovery the current session,
    /// depending on the kind of interruption.
    ///
    /// The actual delay is a randomized value between 0 and this value. This randomization might
    /// help avoid a load spike on the cluster due to simultaneous reconnections, should one of
    /// the active servers be stopped. Note that this delay is only applied before the first reconnection:
    /// should such reconnection fail, only the setting of `setRetryDelay()` will be applied.
    ///
    /// 100 (0.1 seconds)
    ///
    /// This value can be set and changed at any time.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "firstRetryMaxDelay" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `first_retry_max_delay`: The max time (in milliseconds) to wait before trying a new connection.
    ///
    /// # Raises
    ///
    /// * `IllegalArgumentException`: if a negative or zero value is configured
    pub fn set_first_retry_max_delay(
        &mut self,
        first_retry_max_delay: u64,
    ) -> Result<(), IllegalArgumentException> {
        if first_retry_max_delay == 0 {
            return Err(IllegalArgumentException::new(
                "First retry max delay cannot be zero",
            ));
        }

        self.first_retry_max_delay = first_retry_max_delay;
        Ok(())
    }

    /// Setter method that can be used to disable/enable the Stream-Sense algorithm and to force
    /// the client to use a fixed transport or a fixed combination of a transport and a connection
    /// type. When a combination is specified the Stream-Sense algorithm is completely disabled.
    ///
    /// The method can be used to switch between streaming and polling connection types and between
    /// HTTP and WebSocket transports.
    ///
    /// In some cases, the requested status may not be reached, because of connection or environment
    /// problems. In that case the client will continuously attempt to reach the configured status.
    ///
    /// Note that if the Stream-Sense algorithm is disabled, the client may still enter the "CONNECTED:STREAM-SENSING"
    /// status; however, in that case, if it eventually finds out that streaming is not possible,
    /// no recovery will be tried.
    ///
    /// None (full Stream-Sense enabled).
    ///
    /// This method can be called at any time. If called while the client is connecting or connected
    /// it will instruct to switch connection type to match the given configuration.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "forcedTransport" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `forced_transport`:
    ///   - `None`: the Stream-Sense algorithm is enabled and the client will automatically connect
    ///     using the most appropriate transport and connection type among those made possible by
    ///     the environment.
    ///   - `"WS"`: the Stream-Sense algorithm is enabled as in the `None` case but the client will
    ///     only use WebSocket based connections. If a connection over WebSocket is not possible
    ///     because of the environment the client will not connect at all.
    ///   - `"HTTP"`: the Stream-Sense algorithm is enabled as in the `None` case but the client
    ///     will only use HTTP based connections. If a connection over HTTP is not possible because
    ///     of the environment the client will not connect at all.
    ///   - `"WS-STREAMING"`: the Stream-Sense algorithm is disabled and the client will only connect
    ///     on Streaming over WebSocket. If Streaming over WebSocket is not possible because of
    ///     the environment the client will not connect at all.
    ///   - `"HTTP-STREAMING"`: the Stream-Sense algorithm is disabled and the client will only
    ///     connect on Streaming over HTTP. If Streaming over HTTP is not possible because of the
    ///     browser/environment the client will not connect at all.
    ///   - `"WS-POLLING"`: the Stream-Sense algorithm is disabled and the client will only connect
    ///     on Polling over WebSocket. If Polling over WebSocket is not possible because of the
    ///     environment the client will not connect at all.
    ///   - `"HTTP-POLLING"`: the Stream-Sense algorithm is disabled and the client will only connect
    ///     on Polling over HTTP. If Polling over HTTP is not possible because of the environment
    ///     the client will not connect at all.
    ///
    /// # Raises
    ///
    /// * `IllegalArgumentException`: if the given value is not in the list of the admitted ones.
    pub fn set_forced_transport(&mut self, forced_transport: Option<Transport>) {
        self.forced_transport = forced_transport;
    }

    /// Setter method that enables/disables the setting of extra HTTP headers to all the request
    /// performed to the Lightstreamer server by the client.
    ///
    /// Note that the Content-Type header is reserved by the client library itself, while other
    /// headers might be refused by the environment and others might cause the connection to the
    /// server to fail.
    ///
    /// For instance, you cannot use this method to specify custom cookies to be sent to Lightstreamer
    /// Server; leverage `LightstreamerClient.addCookies()` instead. The use of custom headers
    /// might also cause the client to send an OPTIONS request to the server before opening the
    /// actual connection.
    ///
    /// None (meaning no extra headers are sent).
    ///
    /// This setting should be performed before calling the `LightstreamerClient.connect()` method.
    /// However, the value can be changed at any time: the supplied value will be used for the
    /// next HTTP request or WebSocket establishment.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "httpExtraHeaders" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `http_extra_headers`: a Map object containing header-name header-value pairs. `None`
    ///   can be specified to avoid extra headers to be sent.
    pub fn set_http_extra_headers(&mut self, http_extra_headers: Option<HashMap<String, String>>) {
        self.http_extra_headers = http_extra_headers;
    }

    /// Setter method that enables/disables a restriction on the forwarding of the extra http headers
    /// specified through `setHttpExtraHeaders()`. If true, said headers will only be sent during
    /// the session creation process (and thus will still be available to the metadata adapter
    /// notifyUser method) but will not be sent on following requests. On the contrary, when set
    /// to true, the specified extra headers will be sent to the server on every request.
    ///
    /// false
    ///
    /// This setting should be performed before calling the `LightstreamerClient.connect()` method.
    /// However, the value can be changed at any time: the supplied value will be used for the
    /// next HTTP request or WebSocket establishment.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "httpExtraHeadersOnSessionCreationOnly" on any `ClientListener` listening
    /// to the related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `http_extra_headers_on_session_creation_only`: `true`/`false` to enable/disable the restriction
    ///   on extra headers forwarding.
    pub fn set_http_extra_headers_on_session_creation_only(
        &mut self,
        http_extra_headers_on_session_creation_only: bool,
    ) {
        self.http_extra_headers_on_session_creation_only =
            http_extra_headers_on_session_creation_only;
    }

    /// Setter method that sets the maximum time the Server is allowed to wait for any data to
    /// be sent in response to a polling request, if none has accumulated at request time. Setting
    /// this time to a nonzero value and the polling interval to zero leads to an "asynchronous
    /// polling" behavior, which, on low data rates, is very similar to the streaming case. Setting
    /// this time to zero and the polling interval to a nonzero value, on the other hand, leads
    /// to a classical "synchronous polling".
    ///
    /// Note that the Server may, in some cases, delay the answer for more than the supplied time,
    /// to protect itself against a high polling rate or because of bandwidth restrictions. Also,
    /// the Server may impose an upper limit on the wait time, in order to be able to check for
    /// client-side connection drops.
    ///
    /// 19000 (19 seconds).
    ///
    /// The idle timeout should be set before calling the `LightstreamerClient.connect()` method.
    /// However, the value can be changed at any time: the supplied value will be used for the
    /// next polling request.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "idleTimeout" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `idle_timeout`: The time (in milliseconds) the Server is allowed to wait for data to
    ///   send upon polling requests.
    ///
    /// # Raises
    ///
    /// * `IllegalArgumentException`: if a negative value is configured
    pub fn set_idle_timeout(&mut self, idle_timeout: u64) -> Result<(), IllegalArgumentException> {
        if idle_timeout == 0 {
            return Err(IllegalArgumentException::new("Idle timeout cannot be zero"));
        }

        self.idle_timeout = idle_timeout;
        Ok(())
    }

    /// Setter method that sets the interval between two keepalive packets to be sent by Lightstreamer
    /// Server on a stream connection when no actual data is being transmitted. The Server may,
    /// however, impose a lower limit on the keepalive interval, in order to protect itself. Also,
    /// the Server may impose an upper limit on the keepalive interval, in order to be able to
    /// check for client-side connection drops. If 0 is specified, the interval will be decided
    /// by the Server.
    ///
    /// 0 (meaning that the Server will send keepalive packets based on its own configuration).
    ///
    /// The keepalive interval should be set before calling the `LightstreamerClient.connect()`
    /// method. However, the value can be changed at any time: the supplied value will be used
    /// for the next streaming connection (either a bind or a brand new session). Note that, after
    /// a connection, the value may be changed to the one imposed by the Server.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "keepaliveInterval" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `keepalive_interval`: the keepalive interval time (in milliseconds) to set, or 0.
    ///
    /// # Raises
    ///
    /// * `IllegalArgumentException`: if a negative value is configured
    ///
    /// See also `setStalledTimeout()`
    ///
    /// See also `setReconnectTimeout()`
    pub fn set_keepalive_interval(
        &mut self,
        keepalive_interval: u64,
    ) -> Result<(), IllegalArgumentException> {
        if keepalive_interval == 0 {
            self.keepalive_interval = keepalive_interval;
            return Ok(());
        }

        if keepalive_interval < self.stalled_timeout || keepalive_interval < self.reconnect_timeout
        {
            return Err(IllegalArgumentException::new(
                "Keepalive interval should be greater than or equal to stalled timeout and reconnect timeout",
            ));
        }

        self.keepalive_interval = keepalive_interval;
        Ok(())
    }

    /// Setter method that sets the polling interval used for polling connections. The client
    /// switches from the default streaming mode to polling mode when the client network infrastructure
    /// does not allow streaming. Also, polling mode can be forced by calling `setForcedTransport()`
    /// with "WS-POLLING" or "HTTP-POLLING" as parameter.
    ///
    /// The polling interval affects the rate at which polling requests are issued. It is the time
    /// between the start of a polling request and the start of the next request. However, if the
    /// polling interval expires before the first polling request has returned, then the second
    /// polling request is delayed. This may happen, for instance, when the Server delays the answer
    /// because of the idle timeout setting. In any case, the polling interval allows for setting
    /// an upper limit on the polling frequency.
    ///
    /// The Server does not impose a lower limit on the client polling interval. However, in some
    /// cases, it may protect itself against a high polling rate by delaying its answer. Network
    /// limitations and configured bandwidth limits may also lower the polling rate, despite of
    /// the client polling interval.
    ///
    /// The Server may, however, impose an upper limit on the polling interval, in order to be
    /// able to promptly detect terminated polling request sequences and discard related session
    /// information.
    ///
    /// 0 (pure "asynchronous polling" is configured).
    ///
    /// The polling interval should be set before calling the `LightstreamerClient.connect()` method.
    /// However, the value can be changed at any time: the supplied value will be used for the
    /// next polling request.
    ///
    /// Note that, after each polling request, the value may be changed to the one imposed by the
    /// Server.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "pollingInterval" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `polling_interval`: The time (in milliseconds) between subsequent polling requests. Zero
    ///   is a legal value too, meaning that the client will issue a new polling request as soon
    ///   as a previous one has returned.
    ///
    /// # Raises
    ///
    /// * `IllegalArgumentException`: if a negative value is configured
    pub fn set_polling_interval(
        &mut self,
        polling_interval: u64,
    ) -> Result<(), IllegalArgumentException> {
        if polling_interval == 0 {
            self.polling_interval = polling_interval;
            return Ok(());
        }

        if polling_interval < self.idle_timeout {
            return Err(IllegalArgumentException::new(
                "Polling interval should be greater than or equal to idle timeout",
            ));
        }

        self.polling_interval = polling_interval;
        Ok(())
    }

    /// Setter method that configures the coordinates to a proxy server to be used to connect
    /// to the Lightstreamer Server.
    ///
    /// None (meaning not to pass through a proxy).
    ///
    /// This value can be set and changed at any time. The supplied value will be used for the
    /// next connection attempt.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "proxy" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `proxy`: The proxy configuration. Specify `None` to avoid using a proxy.
    pub fn set_proxy(&mut self, proxy: Option<Proxy>) {
        self.proxy = proxy;
    }

    /// Setter method that sets the time the client, after entering "STALLED" status, is allowed
    /// to keep waiting for a keepalive packet or any data on a stream connection, before disconnecting
    /// and trying to reconnect to the Server. The new connection may be either the opening of
    /// a new session or an attempt to recovery the current session, depending on the kind of
    /// interruption.
    ///
    /// 3000 (3 seconds).
    ///
    /// This value can be set and changed at any time.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "reconnectTimeout" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `reconnect_timeout`: The idle time (in milliseconds) allowed in "STALLED" status before
    ///   trying to reconnect to the Server.
    ///
    /// # Raises
    ///
    /// * `IllegalArgumentException`: if a negative or zero value is configured
    ///
    /// See also `setStalledTimeout()`
    ///
    /// See also `setKeepaliveInterval()`
    pub fn set_reconnect_timeout(
        &mut self,
        reconnect_timeout: u64,
    ) -> Result<(), IllegalArgumentException> {
        if reconnect_timeout == 0 {
            return Err(IllegalArgumentException::new(
                "Reconnect timeout cannot be zero",
            ));
        }
        self.reconnect_timeout = reconnect_timeout;
        Ok(())
    }

    /// Setter method that sets the maximum bandwidth expressed in kilobits/s that can be consumed
    /// for the data coming from Lightstreamer Server. A limit on bandwidth may already be posed
    /// by the Metadata Adapter, but the client can furtherly restrict this limit. The limit applies
    /// to the bytes received in each streaming or polling connection.
    ///
    /// Bandwidth Control is an optional feature, available depending on Edition and License Type.
    /// To know what features are enabled by your license, please see the License tab of the Monitoring
    /// Dashboard (by default, available at /dashboard).
    ///
    /// "unlimited"
    ///
    /// The bandwidth limit can be set and changed at any time. If a connection is currently active,
    /// the bandwidth limit for the connection is changed on the fly. Remember that the Server
    /// may apply a different limit.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "requestedMaxBandwidth" on any `ClientListener` listening to the related
    /// `LightstreamerClient`. Moreover, upon any change or attempt to change the limit, the Server
    /// will notify the client and such notification will be received through a call to `ClientListener.onPropertyChange()`
    /// with argument "realMaxBandwidth" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `max_bandwidth`: A decimal number, which represents the maximum bandwidth requested for
    ///   the streaming or polling connection expressed in kbps (kilobits/sec). The string "unlimited"
    ///   is also allowed, to mean that the maximum bandwidth can be entirely decided on the Server
    ///   side (the check is case insensitive).
    ///
    /// # Raises
    ///
    /// * `IllegalArgumentException`: if a negative, zero, or a not-number value (excluding special
    ///   values) is passed.
    ///
    /// See also `get_real_max_bandwidth()`
    pub fn set_requested_max_bandwidth(
        &mut self,
        max_bandwidth: Option<f64>,
    ) -> Result<(), IllegalArgumentException> {
        if let Some(bandwidth) = max_bandwidth {
            if bandwidth <= 0.0 {
                return Err(IllegalArgumentException::new(
                    "Maximum bandwidth should be a positive number or 'unlimited'",
                ));
            }
        }

        self.requested_max_bandwidth = max_bandwidth;
        Ok(())
    }

    /// Setter method that sets
    ///
    /// 1. the minimum time to wait before trying a new connection to the Server in case the previous
    ///    one failed for any reason; and
    /// 2. the maximum time to wait for a response to a request before dropping the connection
    ///    and trying with a different approach.
    ///
    /// Enforcing a delay between reconnections prevents strict loops of connection attempts when
    /// these attempts always fail immediately because of some persisting issue. This applies both
    /// to reconnections aimed at opening a new session and to reconnections aimed at attempting
    /// a recovery of the current session.
    ///
    /// Note that the delay is calculated from the moment the effort to create a connection is
    /// made, not from the moment the failure is detected. As a consequence, when a working connection
    /// is interrupted, this timeout is usually already consumed and the new attempt can be immediate
    /// (except that `ConnectionOptions.setFirstRetryMaxDelay()` will apply in this case). As another
    /// consequence, when a connection attempt gets no answer and times out, the new attempt will
    /// be immediate.
    ///
    /// As a timeout on unresponsive connections, it is applied in these cases:
    ///
    /// - Streaming: Applied on any attempt to setup the streaming connection. If after the timeout
    ///   no data has arrived on the stream connection, the client may automatically switch transport
    ///   or may resort to a polling connection.
    /// - Polling and pre-flight requests: Applied on every connection. If after the timeout no
    ///   data has arrived on the polling connection, the entire connection process restarts from
    ///   scratch.
    ///
    /// This setting imposes only a minimum delay. In order to avoid network congestion, the library
    /// may use a longer delay if the issue preventing the establishment of a session persists.
    ///
    /// 4000 (4 seconds).
    ///
    /// This value can be set and changed at any time.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "retryDelay" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `retry_delay`: The time (in milliseconds) to wait before trying a new connection.
    ///
    /// # Raises
    ///
    /// * `IllegalArgumentException`: if a negative or zero value is configured
    ///
    /// See also `setFirstRetryMaxDelay()`
    pub fn set_retry_delay(&mut self, retry_delay: u64) -> Result<(), IllegalArgumentException> {
        if retry_delay == 0 {
            return Err(IllegalArgumentException::new("Retry delay cannot be zero"));
        }

        self.retry_delay = retry_delay;
        Ok(())
    }

    /// Setter method that enables/disables the reverse-heartbeat mechanism by setting the heartbeat
    /// interval. If the given value (expressed in milliseconds) equals 0 then the reverse-heartbeat
    /// mechanism will be disabled; otherwise if the given value is greater than 0 the mechanism
    /// will be enabled with the specified interval.
    ///
    /// When the mechanism is active, the client will ensure that there is at most the specified
    /// interval between a control request and the following one, by sending empty control requests
    /// (the "reverse heartbeats") if necessary.
    ///
    /// This can serve various purposes:
    ///
    /// 1. Preventing the communication infrastructure from closing an inactive socket that is ready
    ///    for reuse for more HTTP control requests, to avoid connection reestablishment overhead.
    ///    However it is not guaranteed that the connection will be kept open,as the underlying
    ///    TCP implementation may open a new socket each time a HTTP request needs to be sent.
    ///    Note that this will be done only when a session is in place.
    /// 2. Allowing the Server to detect when a streaming connection or Websocket is interrupted
    ///    but not closed. In these cases, the client eventually closes the connection, but the
    ///    Server cannot see that (the connection remains "half-open") and just keeps trying to
    ///    write. This is done by notifying the timeout to the Server upon each streaming request.
    ///    For long polling, the `setIdleTimeout()` setting has a similar function.
    /// 3. Allowing the Server to detect cases in which the client has closed a connection in HTTP
    ///    streaming, but the socket is kept open by some intermediate node, which keeps consuming
    ///    the response. This is also done by notifying the timeout to the Server upon each streaming
    ///    request, whereas, for long polling, the `setIdleTimeout()` setting has a similar function.
    ///
    /// 0 (meaning that the mechanism is disabled).
    ///
    /// This setting should be performed before calling the `LightstreamerClient.connect()` method.
    /// However, the value can be changed at any time: the setting will be obeyed immediately,
    /// unless a higher heartbeat frequency was notified to the Server for the current connection.
    /// The setting will always be obeyed upon the next connection (either a bind or a brand new
    /// session).
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "reverseHeartbeatInterval" on any `ClientListener` listening to the related
    /// `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `reverse_heartbeat_interval`: the interval, expressed in milliseconds, between subsequent
    ///   reverse-heartbeats, or 0.
    ///
    /// # Raises
    ///
    /// * `IllegalArgumentException`: if a negative value is configured
    pub fn set_reverse_heartbeat_interval(
        &mut self,
        reverse_heartbeat_interval: u64,
    ) -> Result<(), IllegalArgumentException> {
        if reverse_heartbeat_interval == 0 {
            self.reverse_heartbeat_interval = reverse_heartbeat_interval;
            return Ok(());
        }

        if reverse_heartbeat_interval < self.retry_delay {
            return Err(IllegalArgumentException::new(
                "Reverse heartbeat interval should be greater than or equal to retry delay",
            ));
        }

        self.reverse_heartbeat_interval = reverse_heartbeat_interval;
        Ok(())
    }

    /// Setter method that can be used to disable/enable the automatic handling of server instance
    /// address that may be returned by the Lightstreamer server during session creation.
    ///
    /// In fact, when a Server cluster is in place, the Server address specified through `ConnectionDetails.setServerAddress()`
    /// can identify various Server instances; in order to ensure that all requests related to
    /// a session are issued to the same Server instance, the Server can answer to the session
    /// opening request by providing an address which uniquely identifies its own instance.
    ///
    /// Setting this value to true permits to ignore that address and to always connect through
    /// the address supplied in `setServerAddress`. This may be needed in a test environment,
    /// if the Server address specified is actually a local address to a specific Server instance
    /// in the cluster.
    ///
    /// Server Clustering is an optional feature, available depending on Edition and License Type.
    /// To know what features are enabled by your license, please see the License tab of the Monitoring
    /// Dashboard (by default, available at /dashboard).
    ///
    /// false.
    ///
    /// This method can be called at any time. If called while connected, it will be applied when
    /// the next session creation request is issued.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "serverInstanceAddressIgnored" on any `ClientListener` listening to the
    /// related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `server_instance_address_ignored`: `true` or `false`, to ignore or not the server instance
    ///   address sent by the server.
    ///
    /// See also `ConnectionDetails.setServerAddress()`
    pub fn set_server_instance_address_ignored(&mut self, server_instance_address_ignored: bool) {
        self.server_instance_address_ignored = server_instance_address_ignored;
    }

    /// Setter method that sets the maximum time allowed for attempts to recover the current session
    /// upon an interruption, after which a new session will be created. If the given value (expressed
    /// in milliseconds) equals 0, then any attempt to recover the current session will be prevented
    /// in the first place.
    ///
    /// In fact, in an attempt to recover the current session, the client will periodically try
    /// to access the Server at the address related with the current session. In some cases, this
    /// timeout, by enforcing a fresh connection attempt, may prevent an infinite sequence of unsuccessful
    /// attempts to access the Server.
    ///
    /// Note that, when the Server is reached, the recovery may fail due to a Server side timeout
    /// on the retention of the session and the updates sent. In that case, a new session will
    /// be created anyway. A setting smaller than the Server timeouts may prevent such useless
    /// failures, but, if too small, it may also prevent successful recovery in some cases.
    ///
    /// 15000 (15 seconds).
    ///
    /// This value can be set and changed at any time.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "sessionRecoveryTimeout" on any `ClientListener` listening to the related
    /// `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `session_recovery_timeout`: The maximum time allowed for recovery attempts, expressed
    ///   in milliseconds, including 0.
    ///
    /// # Raises
    ///
    /// * `IllegalArgumentException`: if a negative value is passed.
    pub fn set_session_recovery_timeout(
        &mut self,
        session_recovery_timeout: u64,
    ) -> Result<(), IllegalArgumentException> {
        if session_recovery_timeout == 0 {
            self.session_recovery_timeout = session_recovery_timeout;
            return Ok(());
        }

        if session_recovery_timeout < self.retry_delay {
            return Err(IllegalArgumentException::new(
                "Session recovery timeout should be greater than or equal to retry delay",
            ));
        }

        self.session_recovery_timeout = session_recovery_timeout;
        Ok(())
    }

    /// Setter method that turns on or off the slowing algorithm. This heuristic algorithm tries
    /// to detect when the client CPU is not able to keep the pace of the events sent by the Server
    /// on a streaming connection. In that case, an automatic transition to polling is performed.
    ///
    /// In polling, the client handles all the data before issuing the next poll, hence a slow
    /// client would just delay the polls, while the Server accumulates and merges the events and
    /// ensures that no obsolete data is sent.
    ///
    /// Only in very slow clients, the next polling request may be so much delayed that the Server
    /// disposes the session first, because of its protection timeouts. In this case, a request
    /// for a fresh session will be reissued by the client and this may happen in cycle.
    ///
    /// false.
    ///
    /// This setting should be performed before calling the `LightstreamerClient.connect()` method.
    /// However, the value can be changed at any time: the supplied value will be used for the
    /// next streaming connection (either a bind or a brand new session).
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "slowingEnabled" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `slowing_enabled`: `true` or `false`, to enable or disable the heuristic algorithm that
    ///   lowers the item update frequency.
    pub fn set_slowing_enabled(&mut self, slowing_enabled: bool) {
        self.slowing_enabled = slowing_enabled;
    }

    /// Setter method that sets the extra time the client is allowed to wait when an expected keepalive
    /// packet has not been received on a stream connection (and no actual data has arrived), before
    /// entering the "STALLED" status.
    ///
    /// 2000 (2 seconds).
    ///
    /// This value can be set and changed at any time.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "stalledTimeout" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `stalled_timeout`: The idle time (in milliseconds) allowed before entering the "STALLED"
    ///   status.
    ///
    /// # Raises
    ///
    /// * `IllegalArgumentException`: if a negative or zero value is configured
    ///
    /// See also `setReconnectTimeout()`
    ///
    /// See also `setKeepaliveInterval()`
    pub fn set_stalled_timeout(
        &mut self,
        stalled_timeout: u64,
    ) -> Result<(), IllegalArgumentException> {
        if stalled_timeout == 0 {
            return Err(IllegalArgumentException::new(
                "Stalled timeout cannot be zero",
            ));
        }

        if stalled_timeout >= self.keepalive_interval {
            return Err(IllegalArgumentException::new(
                "Stalled timeout should be less than keepalive interval",
            ));
        }

        if stalled_timeout >= self.reconnect_timeout {
            return Err(IllegalArgumentException::new(
                "Stalled timeout should be less than reconnect timeout",
            ));
        }

        self.stalled_timeout = stalled_timeout;

        Ok(())
    }

    /// Returns whether the client is configured for polling mode.
    pub fn is_polling(&self) -> bool {
        self.polling
    }

    /// Setter method that configures the client for polling mode.
    ///
    /// In polling mode, the client will open polling connections instead of streaming connections.
    /// This can be useful in environments where streaming connections are not supported or not
    /// recommended.
    ///
    /// If `polling` is set to `true`, the following settings will be automatically configured:
    /// - `polling_interval` will be set to 0 (asynchronous polling)
    /// - `idle_timeout` will be set to 19000 (19 seconds)
    ///
    /// # Parameters
    ///
    /// * `polling`: `true` to enable polling mode, `false` to disable it.
    pub fn set_polling(&mut self, polling: bool) {
        self.polling = polling;

        if polling {
            self.polling_interval = 0;
            self.idle_timeout = 19000;
        }
    }

    /// Inquiry method that gets the time-to-live for a request, expressed in milliseconds.
    ///
    /// If the time-to-live is exceeded and the request is still pending, it will be aborted by
    /// the client library.
    ///
    /// # Returns
    ///
    /// The time-to-live for a request, expressed in milliseconds. If `None`, the request will
    /// be kept until completion.
    pub fn get_ttl_millis(&self) -> Option<u64> {
        self.ttl_millis
    }

    /// Setter method that sets the time-to-live for a request, expressed in milliseconds.
    ///
    /// If the time-to-live is exceeded and the request is still pending, it will be aborted by
    /// the client library.
    ///
    /// # Parameters
    ///
    /// * `ttl_millis`: The time-to-live for a request, expressed in milliseconds. If `None`, the
    ///   request will be kept until completion.
    pub fn set_ttl_millis(&mut self, ttl_millis: Option<u64>) {
        self.ttl_millis = ttl_millis;
    }

    /// Inquiry method that gets the list of supported "diff" formats accepted for the indication
    /// of update values.
    ///
    /// The protocol allows the Server to choose among a few "diff" algorithms to express new values
    /// as differences from previous values. This setting allows the client to restrict the set
    /// of accepted formats.
    ///
    /// # Returns
    ///
    /// The list of supported "diff" formats, or `None` if all formats are accepted.
    pub fn get_supported_diffs(&self) -> Option<&String> {
        self.supported_diffs.as_ref()
    }

    /// Setter method that sets the list of supported "diff" formats accepted for the indication
    /// of update values.
    ///
    /// The protocol allows the Server to choose among a few "diff" algorithms to express new values
    /// as differences from previous values. This setting allows the client to restrict the set
    /// of accepted formats.
    ///
    /// # Parameters
    ///
    /// * `supported_diffs`: The list of supported "diff" formats, or `None` to accept all formats.
    ///   The list should be a comma-separated string of format tags.
    pub fn set_supported_diffs(&mut self, supported_diffs: Option<String>) {
        self.supported_diffs = supported_diffs;
    }
}

impl Debug for ConnectionOptions {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionOptions")
            .field("content_length", &self.content_length)
            .field("first_retry_max_delay", &self.first_retry_max_delay)
            .field("forced_transport", &self.forced_transport)
            .field("http_extra_headers", &self.http_extra_headers)
            .field(
                "http_extra_headers_on_session_creation_only",
                &self.http_extra_headers_on_session_creation_only,
            )
            .field("idle_timeout", &self.idle_timeout)
            .field("keepalive_interval", &self.keepalive_interval)
            .field("polling_interval", &self.polling_interval)
            .field("proxy", &self.proxy)
            .field("real_max_bandwidth", &self.real_max_bandwidth)
            .field("reconnect_timeout", &self.reconnect_timeout)
            .field("requested_max_bandwidth", &self.requested_max_bandwidth)
            .field("retry_delay", &self.retry_delay)
            .field(
                "reverse_heartbeat_interval",
                &self.reverse_heartbeat_interval,
            )
            .field(
                "server_instance_address_ignored",
                &self.server_instance_address_ignored,
            )
            .field("session_recovery_timeout", &self.session_recovery_timeout)
            .field("slowing_enabled", &self.slowing_enabled)
            .field("stalled_timeout", &self.stalled_timeout)
            .finish()
    }
}

impl Default for ConnectionOptions {
    fn default() -> Self {
        Self {
            content_length: None,
            first_retry_max_delay: 0,
            forced_transport: None,
            http_extra_headers: None,
            http_extra_headers_on_session_creation_only: false,
            idle_timeout: 19000,
            keepalive_interval: 0,
            polling_interval: 0,
            proxy: None,
            real_max_bandwidth: None,
            reconnect_timeout: 3000,
            _reduce_head: false,
            requested_max_bandwidth: None,
            retry_delay: 4000,
            reverse_heartbeat_interval: 0,
            send_sync: false,
            server_instance_address_ignored: false,
            session_recovery_timeout: 15000,
            slowing_enabled: false,
            stalled_timeout: 2000,
            polling: false,
            ttl_millis: None,
            supported_diffs: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_connection_options() {
        let options = ConnectionOptions::new();

        // Verify default values
        assert_eq!(options.get_content_length(), None);
        assert_eq!(options.get_first_retry_max_delay(), 100);
        assert_eq!(options.get_forced_transport(), None);
        assert_eq!(options.get_http_extra_headers(), None);
        assert!(!options.is_http_extra_headers_on_session_creation_only());
        assert_eq!(options.get_idle_timeout(), 19000);
        assert_eq!(options.get_keepalive_interval(), 0);
        assert_eq!(options.get_polling_interval(), 0);
        assert_eq!(options.get_reconnect_timeout(), 3000);
        assert_eq!(options.get_retry_delay(), 4000);
        assert_eq!(options.get_reverse_heartbeat_interval(), 0);
        assert!(!options.is_server_instance_address_ignored());
        assert_eq!(options.get_session_recovery_timeout(), 15000);
        assert!(!options.is_slowing_enabled());
        assert_eq!(options.get_stalled_timeout(), 2000);
        assert!(options.get_send_sync());
    }

    #[test]
    fn test_set_content_length() {
        let mut options = ConnectionOptions::new();

        // Test valid content length
        assert!(options.set_content_length(10000).is_ok());
        assert_eq!(options.get_content_length(), Some(10000));

        // Test invalid (zero) content length
        assert!(options.set_content_length(0).is_err());
    }

    #[test]
    fn test_set_first_retry_max_delay() {
        let mut options = ConnectionOptions::new();

        // Test valid delay
        assert!(options.set_first_retry_max_delay(500).is_ok());
        assert_eq!(options.get_first_retry_max_delay(), 500);

        // Test invalid (zero) delay
        assert!(options.set_first_retry_max_delay(0).is_err());
    }

    #[test]
    fn test_set_forced_transport() {
        let mut options = ConnectionOptions::new();

        // Test setting different transport types
        options.set_forced_transport(Some(Transport::WsStreaming));
        assert_eq!(
            options.get_forced_transport(),
            Some(&Transport::WsStreaming)
        );

        options.set_forced_transport(Some(Transport::HttpStreaming));
        assert_eq!(
            options.get_forced_transport(),
            Some(&Transport::HttpStreaming)
        );

        options.set_forced_transport(Some(Transport::WsPolling));
        assert_eq!(options.get_forced_transport(), Some(&Transport::WsPolling));

        options.set_forced_transport(Some(Transport::HttpPolling));
        assert_eq!(
            options.get_forced_transport(),
            Some(&Transport::HttpPolling)
        );

        options.set_forced_transport(Some(Transport::Ws));
        assert_eq!(options.get_forced_transport(), Some(&Transport::Ws));

        options.set_forced_transport(Some(Transport::Http));
        assert_eq!(options.get_forced_transport(), Some(&Transport::Http));

        // Test setting None
        options.set_forced_transport(None);
        assert_eq!(options.get_forced_transport(), None);
    }

    #[test]
    fn test_set_http_extra_headers() {
        let mut options = ConnectionOptions::new();

        // Test setting headers
        let mut headers = HashMap::new();
        headers.insert("X-Custom-Header".to_string(), "Value".to_string());
        headers.insert("X-Another-Header".to_string(), "AnotherValue".to_string());

        options.set_http_extra_headers(Some(headers.clone()));
        assert_eq!(options.get_http_extra_headers().unwrap(), &headers);

        // Test setting None
        options.set_http_extra_headers(None);
        assert_eq!(options.get_http_extra_headers(), None);
    }

    #[test]
    fn test_set_http_extra_headers_on_session_creation_only() {
        let mut options = ConnectionOptions::new();

        // Test setting to true
        options.set_http_extra_headers_on_session_creation_only(true);
        assert!(options.is_http_extra_headers_on_session_creation_only());

        // Test setting to false
        options.set_http_extra_headers_on_session_creation_only(false);
        assert!(!options.is_http_extra_headers_on_session_creation_only());
    }

    #[test]
    fn test_set_idle_timeout() {
        let mut options = ConnectionOptions::new();

        // Test valid timeout
        assert!(options.set_idle_timeout(15000).is_ok());
        assert_eq!(options.get_idle_timeout(), 15000);

        // Test invalid (zero) timeout
        assert!(options.set_idle_timeout(0).is_err());
    }

    #[test]
    fn test_set_keepalive_interval() {
        let mut options = ConnectionOptions::new();

        // Test valid interval
        assert!(options.set_keepalive_interval(5000).is_ok());
        assert_eq!(options.get_keepalive_interval(), 5000);

        // Test zero interval (special case - valid)
        assert!(options.set_keepalive_interval(0).is_ok());
        assert_eq!(options.get_keepalive_interval(), 0);
    }

    #[test]
    fn test_set_polling_interval() {
        let mut options = ConnectionOptions::new();

        assert!(options.set_idle_timeout(2000).is_ok());
        assert!(options.set_polling_interval(3000).is_ok());
        assert_eq!(options.get_polling_interval(), 3000);
        assert!(options.set_polling_interval(0).is_ok());
        assert_eq!(options.get_polling_interval(), 0);
        assert!(options.set_idle_timeout(19000).is_ok());
        assert!(options.set_polling_interval(10000).is_err());
    }

    #[test]
    fn test_set_reconnect_timeout() {
        let mut options = ConnectionOptions::new();

        // Test valid timeout
        assert!(options.set_reconnect_timeout(5000).is_ok());
        assert_eq!(options.get_reconnect_timeout(), 5000);

        // Test invalid (zero) timeout
        assert!(options.set_reconnect_timeout(0).is_err());
    }

    #[test]
    fn test_set_requested_max_bandwidth() {
        let mut options = ConnectionOptions::new();

        // Test valid bandwidth
        assert!(options.set_requested_max_bandwidth(Some(10.5)).is_ok());
        assert_eq!(options.get_requested_max_bandwidth(), Some(10.5));

        // Test invalid (zero) bandwidth
        assert!(options.set_requested_max_bandwidth(Some(0.0)).is_err());

        // Test setting None
        assert!(options.set_requested_max_bandwidth(None).is_ok());
        assert_eq!(options.get_requested_max_bandwidth(), None);
    }

    #[test]
    fn test_set_retry_delay() {
        let mut options = ConnectionOptions::new();

        // Test valid delay
        assert!(options.set_retry_delay(3000).is_ok());
        assert_eq!(options.get_retry_delay(), 3000);

        // Test invalid (zero) delay
        assert!(options.set_retry_delay(0).is_err());
    }

    #[test]
    fn test_set_reverse_heartbeat_interval() {
        let mut options = ConnectionOptions::new();

        // Test valid interval
        assert!(options.set_reverse_heartbeat_interval(5000).is_ok());
        assert_eq!(options.get_reverse_heartbeat_interval(), 5000);

        // Test zero interval (special case - valid)
        assert!(options.set_reverse_heartbeat_interval(0).is_ok());
        assert_eq!(options.get_reverse_heartbeat_interval(), 0);
    }

    #[test]
    fn test_set_server_instance_address_ignored() {
        let mut options = ConnectionOptions::new();

        // Test setting to true
        options.set_server_instance_address_ignored(true);
        assert!(options.is_server_instance_address_ignored());

        // Test setting to false
        options.set_server_instance_address_ignored(false);
        assert!(!options.is_server_instance_address_ignored());
    }

    #[test]
    fn test_set_session_recovery_timeout() {
        let mut options = ConnectionOptions::new();

        // Test valid timeout
        assert!(options.set_session_recovery_timeout(10000).is_ok());
        assert_eq!(options.get_session_recovery_timeout(), 10000);

        // Test zero timeout (special case - valid)
        assert!(options.set_session_recovery_timeout(0).is_ok());
        assert_eq!(options.get_session_recovery_timeout(), 0);
    }

    #[test]
    fn test_set_slowing_enabled() {
        let mut options = ConnectionOptions::new();

        // Test setting to true
        options.set_slowing_enabled(true);
        assert!(options.is_slowing_enabled());

        // Test setting to false
        options.set_slowing_enabled(false);
        assert!(!options.is_slowing_enabled());
    }

    #[test]
    fn test_set_stalled_timeout() {
        let mut options = ConnectionOptions::new();

        assert!(options.set_keepalive_interval(5000).is_ok());
        assert!(options.set_stalled_timeout(1000).is_ok());
        assert_eq!(options.get_stalled_timeout(), 1000);
        assert!(options.set_stalled_timeout(0).is_err());
        assert!(options.set_stalled_timeout(6000).is_err());

        options.set_reconnect_timeout(2000).unwrap();
        assert!(options.set_stalled_timeout(1500).is_ok());
        assert!(options.set_stalled_timeout(2500).is_err());
    }

    #[test]
    fn test_set_polling() {
        let mut options = ConnectionOptions::new();

        // Test setting to true
        options.set_polling(true);
        assert!(options.is_polling());
        assert_eq!(options.get_polling_interval(), 0); // Should be set to 0
        assert_eq!(options.get_idle_timeout(), 19000); // Should be set to 19000

        // Test setting to false
        options.set_polling(false);
        assert!(!options.is_polling());
    }

    #[test]
    fn test_set_ttl_millis() {
        let mut options = ConnectionOptions::new();

        // Test setting value
        options.set_ttl_millis(Some(5000));
        assert_eq!(options.get_ttl_millis(), Some(5000));

        // Test setting None
        options.set_ttl_millis(None);
        assert_eq!(options.get_ttl_millis(), None);
    }

    #[test]
    fn test_set_supported_diffs() {
        let mut options = ConnectionOptions::new();

        // Test setting value
        options.set_supported_diffs(Some("TLCP-diff,JSON-patch".to_string()));
        assert_eq!(
            options.get_supported_diffs().unwrap(),
            "TLCP-diff,JSON-patch"
        );

        // Test setting None
        options.set_supported_diffs(None);
        assert_eq!(options.get_supported_diffs(), None);
    }

    #[test]
    fn test_debug_implementation() {
        let options = ConnectionOptions::new();

        // Test that Debug implementation works without panicking
        let debug_string = format!("{:?}", options);

        // Verify it contains some expected fields
        assert!(debug_string.contains("content_length"));
        assert!(debug_string.contains("first_retry_max_delay"));
        assert!(debug_string.contains("forced_transport"));
        assert!(debug_string.contains("http_extra_headers"));
    }
}
#[test]
fn test_combined_settings() {
    let mut options = ConnectionOptions::new();

    // Test multiple settings together
    options.set_keepalive_interval(5000).unwrap();
    options.set_stalled_timeout(2000).unwrap();
    options.set_reconnect_timeout(3000).unwrap();
    options.set_first_retry_max_delay(100).unwrap();
    options.set_retry_delay(4000).unwrap();

    // Verify all settings were applied correctly
    assert_eq!(options.get_keepalive_interval(), 5000);
    assert_eq!(options.get_stalled_timeout(), 2000);
    assert_eq!(options.get_reconnect_timeout(), 3000);
    assert_eq!(options.get_first_retry_max_delay(), 100);
    assert_eq!(options.get_retry_delay(), 4000);
}
