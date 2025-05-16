use crate::client::ClientListener;
use crate::utils::IllegalArgumentException;
use std::error::Error;
use std::fmt::{self, Debug, Formatter};

/// Used by `LightstreamerClient` to provide a basic connection properties data object.
///
/// Data object that contains the configuration settings needed to connect to a Lightstreamer Server.
///
/// An instance of this class is attached to every `LightstreamerClient` as `LightstreamerClient.connectionDetails`
///
/// See also `LightstreamerClient`
#[derive(Default)]
pub struct ConnectionDetails {
    adapter_set: Option<String>,
    client_ip: Option<String>,
    server_address: Option<String>,
    server_instance_address: Option<String>,
    server_socket_name: Option<String>,
    session_id: Option<String>,
    user: Option<String>,
    password: Option<String>,
    listeners: Vec<Box<dyn ClientListener>>,
}

impl ConnectionDetails {
    /// Inquiry method that gets the name of the Adapter Set (which defines the Metadata Adapter
    /// and one or several Data Adapters) mounted on Lightstreamer Server that supply all the
    /// items used in this application.
    ///
    /// # Returns
    ///
    /// The name of the Adapter Set; returns `None` if no name has been configured, that means
    /// that the "DEFAULT" Adapter Set is used.
    ///
    /// See also `setAdapterSet()`
    pub fn get_adapter_set(&self) -> Option<&String> {
        self.adapter_set.as_ref()
    }

    /// Inquiry method that gets the IP address of this client as seen by the Server which is
    /// serving the current session as the client remote address (note that it may not correspond
    /// to the client host; for instance it may refer to an intermediate proxy). If, upon a new
    /// session, this address changes, it may be a hint that the intermediary network nodes handling
    /// the connection have changed, hence the network capabilities may be different. The library
    /// uses this information to optimize the connection.
    ///
    /// Note that in case of polling or in case rebind requests are needed, subsequent requests
    /// related to the same session may, in principle, expose a different IP address to the Server;
    /// these changes would not be reported.
    ///
    /// If a session is not currently active, `None` is returned; soon after a session is established,
    /// the value may become available; but it is possible that this information is not provided
    /// by the Server and that it will never be available.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "clientIp" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Returns
    ///
    /// A canonical representation of an IP address (it can be either IPv4 or IPv6), or `None`.
    pub fn get_client_ip(&self) -> Option<&String> {
        self.client_ip.as_ref()
    }

    /// Retrieves a reference to the password, if set.
    ///
    /// This method is crucial for accessing sensitive information in a controlled manner. It returns
    /// an immutable reference to the password, encapsulated within an `Option`. The use of `Option`
    /// signifies that the password may or may not be present, thus providing flexibility in scenarios
    /// where a password is optional. By returning a reference, we avoid unnecessary cloning of the
    /// password data, which could have security implications and also incur a performance cost.
    ///
    /// # Returns
    /// An `Option` containing a reference to the password `String` if it exists, or `None` if the
    /// password has not been set. This allows calling code to handle the presence or absence of a
    /// password appropriately without risking exposure of the password itself.
    pub fn get_password(&self) -> Option<&String> {
        self.password.as_ref()
    }

    /// Inquiry method that gets the configured address of Lightstreamer Server.
    ///
    /// # Returns
    ///
    /// The configured address of Lightstreamer Server.
    pub fn get_server_address(&self) -> Option<&String> {
        self.server_address.as_ref()
    }

    /// Inquiry method that gets the server address to be used to issue all requests related to
    /// the current session. In fact, when a Server cluster is in place, the Server address specified
    /// through `setServerAddress()` can identify various Server instances; in order to ensure that
    /// all requests related to a session are issued to the same Server instance, the Server can
    /// answer to the session opening request by providing an address which uniquely identifies
    /// its own instance. When this is the case, this address is returned by the method; otherwise,
    /// `None` is returned.
    ///
    /// Note that the addresses will always have the `http:` or `https:` scheme. In case WebSockets
    /// are used, the specified scheme is internally converted to match the related WebSocket protocol
    /// (i.e. `http` becomes `ws` while `https` becomes `wss`).
    ///
    /// Server Clustering is an optional feature, available depending on Edition and License Type.
    /// To know what features are enabled by your license, please see the License tab of the Monitoring
    /// Dashboard (by default, available at /dashboard).
    ///
    /// The method gives a meaningful answer only when a session is currently active.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "serverInstanceAddress" on any `ClientListener` listening to the related
    /// `LightstreamerClient`.
    ///
    /// # Returns
    ///
    /// Address used to issue all requests related to the current session.
    pub fn get_server_instance_address(&self) -> Option<&String> {
        self.server_instance_address.as_ref()
    }

    /// Inquiry method that gets the instance name of the Server which is serving the current session.
    /// To be more precise, each answering port configured on a Server instance (through a `<http_server>`
    /// or `<https_server>` element in the Server configuration file) can be given a different name;
    /// the name related to the port to which the session opening request has been issued is returned.
    ///
    /// Note that each rebind to the same session can, potentially, reach the Server on a port different
    /// than the one used for the previous request, depending on the behavior of intermediate nodes.
    /// However, the only meaningful case is when a Server cluster is in place and it is configured
    /// in such a way that the port used for all `bind_session` requests differs from the port used
    /// for the initial `create_session` request.
    ///
    /// Server Clustering is an optional feature, available depending on Edition and License Type.
    /// To know what features are enabled by your license, please see the License tab of the Monitoring
    /// Dashboard (by default, available at /dashboard).
    ///
    /// If a session is not currently active, `None` is returned; soon after a session is established,
    /// the value will become available.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "serverSocketName" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Returns
    ///
    /// Name configured for the Server instance which is managing the current session, or `None`.
    pub fn get_server_socket_name(&self) -> Option<&String> {
        self.server_socket_name.as_ref()
    }

    /// Inquiry method that gets the ID associated by the server to this client session.
    ///
    /// The method gives a meaningful answer only when a session is currently active.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "sessionId" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Returns
    ///
    /// ID assigned by the Server to this client session.
    pub fn get_session_id(&self) -> Option<&String> {
        self.session_id.as_ref()
    }

    /// Inquiry method that gets the username to be used for the authentication on Lightstreamer
    /// Server when initiating the session.
    ///
    /// # Returns
    ///
    /// The username to be used for the authentication on Lightstreamer Server; returns `None`
    /// if no user name has been configured.
    pub fn get_user(&self) -> Option<&String> {
        self.user.as_ref()
    }

    /// Creates a new ConnectionDetails object with default values.
    pub fn new(
        server_address: Option<&str>,
        adapter_set: Option<&str>,
        user: Option<&str>,
        password: Option<&str>,
    ) -> Result<ConnectionDetails, Box<dyn Error>> {
        let mut connection_details = ConnectionDetails::default();
        connection_details.set_server_address(server_address.map(|s| s.to_string()))?;
        connection_details.set_adapter_set(adapter_set.map(|s| s.to_string()));
        connection_details.set_user(user.map(|s| s.to_string()));
        connection_details.set_password(password.map(|s| s.to_string()));

        Ok(connection_details)
    }

    /// Setter method that sets the name of the Adapter Set mounted on Lightstreamer Server to
    /// be used to handle all requests in the session.
    ///
    /// An Adapter Set defines the Metadata Adapter and one or several Data Adapters. It is configured
    /// on the server side through an "adapters.xml" file; the name is configured through the "id"
    /// attribute in the `<adapters_conf>` element.
    ///
    /// The default Adapter Set, configured as "DEFAULT" on the Server.
    ///
    /// The Adapter Set name should be set on the `LightstreamerClient.connectionDetails` object
    /// before calling the `LightstreamerClient.connect()` method. However, the value can be changed
    /// at any time: the supplied value will be used for the next time a new session is requested
    /// to the server.
    ///
    /// This setting can also be specified in the `LightstreamerClient` constructor.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "adapterSet" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `adapter_set`: The name of the Adapter Set to be used. A `None` value is equivalent to
    ///   the "DEFAULT" name.
    pub fn set_adapter_set(&mut self, adapter_set: Option<String>) {
        self.adapter_set = Some(adapter_set.unwrap_or("DEFAULT".to_string()));

        // Notify listeners about the property change
        for listener in &self.listeners {
            listener.on_property_change("adapterSet");
        }
    }

    /// Setter method that sets the password to be used for the authentication on Lightstreamer
    /// Server when initiating the session. The Metadata Adapter is responsible for checking the
    /// credentials (username and password).
    ///
    /// If no password is supplied, no password information will be sent at session initiation.
    /// The Metadata Adapter, however, may still allow the session.
    ///
    /// The password should be set on the `LightstreamerClient.connectionDetails` object before
    /// calling the `LightstreamerClient.connect()` method. However, the value can be changed at
    /// any time: the supplied value will be used for the next time a new session is requested to
    /// the server.
    ///
    /// NOTE: The password string will be stored in the current instance. That is necessary in order
    /// to allow automatic reconnection/reauthentication for fail-over. For maximum security, avoid
    /// using an actual private password to authenticate on Lightstreamer Server; rather use a session-id
    /// originated by your web/application server, that can be checked by your Metadata Adapter.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "password" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `password`: The password to be used for the authentication on Lightstreamer Server. The
    ///   password can be `None`.
    ///
    /// See also `setUser()`
    pub fn set_password(&mut self, password: Option<String>) {
        self.password = password;

        // Notify listeners about the property change
        for listener in &self.listeners {
            listener.on_property_change("password");
        }
    }

    /// Setter method that sets the address of Lightstreamer Server.
    ///
    /// Note that the addresses specified must always have the `http:` or `https:` scheme. In case
    /// WebSockets are used, the specified scheme is internally converted to match the related WebSocket
    /// protocol (i.e. `http` becomes `ws` while `https` becomes `wss`).
    ///
    /// WSS/HTTPS is an optional feature, available depending on Edition and License Type. To know
    /// what features are enabled by your license, please see the License tab of the Monitoring
    /// Dashboard (by default, available at /dashboard).
    ///
    /// If no server address is supplied the client will be unable to connect.
    ///
    /// This method can be called at any time. If called while connected, it will be applied when
    /// the next session creation request is issued. This setting can also be specified in the
    /// `LightstreamerClient` constructor.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "serverAddress" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `server_address`: The full address of Lightstreamer Server. A `None` value can also be
    ///   used, to restore the default value.
    ///
    /// An IPv4 or IPv6 can also be used in place of a hostname. Some examples of valid values include:
    ///
    /// - `http://push.mycompany.com`
    /// - `http://push.mycompany.com:8080`
    /// - `http://79.125.7.252`
    /// - `http://[2001:0db8:85a3:0000:0000:8a2e:0370:7334]`
    /// - `http://[2001:0db8:85a3::8a2e:0370:7334]:8080`
    ///
    /// # Raises
    ///
    /// * `IllegalArgumentException`: if the given address is not valid.
    pub fn set_server_address(
        &mut self,
        server_address: Option<String>,
    ) -> Result<(), IllegalArgumentException> {
        // Validate the server address
        if let Some(address) = &server_address {
            if !address.starts_with("http://") && !address.starts_with("https://") {
                return Err(IllegalArgumentException::new(
                    "Invalid server address: must start with http:// or https://",
                ));
            }
        }

        self.server_address = server_address;

        // Notify listeners about the property change
        for listener in &self.listeners {
            listener.on_property_change("serverAddress");
        }

        Ok(())
    }

    /// Setter method that sets the username to be used for the authentication on Lightstreamer
    /// Server when initiating the session. The Metadata Adapter is responsible for checking the
    /// credentials (username and password).
    ///
    /// If no username is supplied, no user information will be sent at session initiation. The
    /// Metadata Adapter, however, may still allow the session.
    ///
    /// The username should be set on the `LightstreamerClient.connectionDetails` object before
    /// calling the `LightstreamerClient.connect()` method. However, the value can be changed at
    /// any time: the supplied value will be used for the next time a new session is requested to
    /// the server.
    ///
    /// A change to this setting will be notified through a call to `ClientListener.onPropertyChange()`
    /// with argument "user" on any `ClientListener` listening to the related `LightstreamerClient`.
    ///
    /// # Parameters
    ///
    /// * `user`: The username to be used for the authentication on Lightstreamer Server. The username
    ///   can be `None`.
    ///
    /// See also `setPassword()`
    pub fn set_user(&mut self, user: Option<String>) {
        self.user = user;

        // Notify listeners about the property change
        for listener in &self.listeners {
            listener.on_property_change("user");
        }
    }

    /// Adds a listener that will receive events related to changes in the `ConnectionDetails`.
    ///
    /// The same listener can be added to multiple instances of `ConnectionDetails`.
    ///
    /// # Parameters
    ///
    /// * `listener`: An object that will receive the events as documented in the `ClientListener`
    ///   interface.
    pub fn add_listener(&mut self, listener: Box<dyn ClientListener>) {
        self.listeners.push(listener);
    }

    /// Removes a listener from the `ConnectionDetails` instance so that it will not receive events
    /// anymore.
    ///
    /// # Parameters
    ///
    /// * `listener`: The listener to be removed.
    pub fn remove_listener(&mut self, _listener: Box<dyn ClientListener>) {
        unimplemented!("Implement mechanism to remove listener from ConnectionDetails.");
        //self.listeners.remove(&listener);
    }
}

impl Debug for ConnectionDetails {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionDetails")
            .field("adapter_set", &self.adapter_set)
            .field("client_ip", &self.client_ip)
            .field("server_address", &self.server_address)
            .field("server_instance_address", &self.server_instance_address)
            .field("server_socket_name", &self.server_socket_name)
            .field("session_id", &self.session_id)
            .field("user", &self.user)
            .field("password", &self.password)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Debug;

    #[derive(Debug)]
    struct MockClientListener {
        property_changes: std::cell::RefCell<Vec<String>>,
    }

    impl MockClientListener {
        fn new() -> Self {
            MockClientListener {
                property_changes: std::cell::RefCell::new(Vec::new()),
            }
        }

        fn get_property_changes(&self) -> Vec<String> {
            self.property_changes.borrow().clone()
        }
    }

    impl ClientListener for MockClientListener {
        fn on_property_change(&self, property: &str) {
            self.property_changes
                .borrow_mut()
                .push(property.to_string());
        }
    }

    #[test]
    fn test_new_connection_details() {
        // Test with all values provided
        let result = ConnectionDetails::new(
            Some("http://test.lightstreamer.com"),
            Some("DEMO"),
            Some("user1"),
            Some("pass1"),
        );
        assert!(result.is_ok());
        let details = result.unwrap();
        assert_eq!(
            details.get_server_address().unwrap(),
            "http://test.lightstreamer.com"
        );
        assert_eq!(details.get_adapter_set().unwrap(), "DEMO");
        assert_eq!(details.get_user().unwrap(), "user1");
        assert_eq!(details.get_password().unwrap(), "pass1");

        // Test with only mandatory values
        let result =
            ConnectionDetails::new(Some("http://test.lightstreamer.com"), None, None, None);
        assert!(result.is_ok());
        let details = result.unwrap();
        assert_eq!(
            details.get_server_address().unwrap(),
            "http://test.lightstreamer.com"
        );
        assert_eq!(details.get_adapter_set().unwrap(), "DEFAULT"); // Default value
        assert_eq!(details.get_user(), None);
        assert_eq!(details.get_password(), None);

        // Test with invalid server address
        let result = ConnectionDetails::new(Some("invalid-url"), None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_set_server_address() {
        let mut details = ConnectionDetails::default();

        // Test valid HTTP URL
        assert!(
            details
                .set_server_address(Some("http://test.lightstreamer.com".to_string()))
                .is_ok()
        );
        assert_eq!(
            details.get_server_address().unwrap(),
            "http://test.lightstreamer.com"
        );

        // Test valid HTTPS URL
        assert!(
            details
                .set_server_address(Some("https://test.lightstreamer.com".to_string()))
                .is_ok()
        );
        assert_eq!(
            details.get_server_address().unwrap(),
            "https://test.lightstreamer.com"
        );

        // Test with port
        assert!(
            details
                .set_server_address(Some("https://test.lightstreamer.com:8080".to_string()))
                .is_ok()
        );
        assert_eq!(
            details.get_server_address().unwrap(),
            "https://test.lightstreamer.com:8080"
        );

        // Test invalid URL (missing http:// or https://)
        assert!(
            details
                .set_server_address(Some("test.lightstreamer.com".to_string()))
                .is_err()
        );

        // Test None value
        assert!(details.set_server_address(None).is_ok());
        assert_eq!(details.get_server_address(), None);
    }

    #[test]
    fn test_set_adapter_set() {
        let mut details = ConnectionDetails::default();

        // Test setting adapter set
        details.set_adapter_set(Some("TEST_ADAPTER".to_string()));
        assert_eq!(details.get_adapter_set().unwrap(), "TEST_ADAPTER");

        // Test setting None (should default to "DEFAULT")
        details.set_adapter_set(None);
        assert_eq!(details.get_adapter_set().unwrap(), "DEFAULT");
    }

    #[test]
    fn test_set_user_and_password() {
        let mut details = ConnectionDetails::default();

        // Test setting user
        details.set_user(Some("test_user".to_string()));
        assert_eq!(details.get_user().unwrap(), "test_user");

        // Test setting None for user
        details.set_user(None);
        assert_eq!(details.get_user(), None);

        // Test setting password
        details.set_password(Some("test_password".to_string()));
        assert_eq!(details.get_password().unwrap(), "test_password");

        // Test setting None for password
        details.set_password(None);
        assert_eq!(details.get_password(), None);
    }

    #[test]
    fn test_property_change_notifications() {
        let mut details = ConnectionDetails::default();
        let listener = Box::new(MockClientListener::new());
        let listener_ref = &*listener as &dyn ClientListener as *const _ as *mut MockClientListener;

        details.add_listener(listener);

        // Change server address and verify notification
        assert!(
            details
                .set_server_address(Some("http://test.lightstreamer.com".to_string()))
                .is_ok()
        );

        // Change adapter set and verify notification
        details.set_adapter_set(Some("TEST_ADAPTER".to_string()));

        // Change user and verify notification
        details.set_user(Some("test_user".to_string()));

        // Change password and verify notification
        details.set_password(Some("test_password".to_string()));

        // Get property changes from the listener
        let changes = unsafe { &*listener_ref }.get_property_changes();

        // Verify all property changes were notified
        assert!(changes.contains(&"serverAddress".to_string()));
        assert!(changes.contains(&"adapterSet".to_string()));
        assert!(changes.contains(&"user".to_string()));
        assert!(changes.contains(&"password".to_string()));
    }

    #[test]
    fn test_default_connection_details() {
        let details = ConnectionDetails::default();

        assert_eq!(details.get_server_address(), None);
        assert_eq!(details.get_adapter_set(), None);
        assert_eq!(details.get_user(), None);
        assert_eq!(details.get_password(), None);
        assert_eq!(details.get_client_ip(), None);
        assert_eq!(details.get_server_instance_address(), None);
        assert_eq!(details.get_server_socket_name(), None);
        assert_eq!(details.get_session_id(), None);
    }
}
