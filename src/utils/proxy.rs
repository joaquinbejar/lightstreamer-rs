/// Simple class representing a Proxy configuration.
///
/// An instance of this class can be used through `ConnectionOptions.setProxy()` to instruct
/// a `LightstreamerClient` to connect to the Lightstreamer Server passing through a proxy.
///
/// # Parameters
///
/// * `proxy_type`: the proxy type
/// * `host`: the proxy host
/// * `port`: the proxy port
/// * `user`: the user name to be used to validate against the proxy. Optional.
/// * `password`: the password to be used to validate against the proxy. Optional.
#[derive(Debug)]
pub struct Proxy {
    proxy_type: ProxyType,
    host: String,
    port: u16,
    user: Option<String>,
    password: Option<String>,
}

impl Proxy {
    /// Creates a new instance of `Proxy`.
    ///
    /// # Parameters
    ///
    /// * `proxy_type`: the proxy type
    /// * `host`: the proxy host
    /// * `port`: the proxy port
    /// * `user`: the user name to be used to validate against the proxy. Optional.
    /// * `password`: the password to be used to validate against the proxy. Optional.
    pub fn new(
        proxy_type: ProxyType,
        host: String,
        port: u16,
        user: Option<String>,
        password: Option<String>,
    ) -> Self {
        Proxy {
            proxy_type,
            host,
            port,
            user,
            password,
        }
    }

    /// Returns the proxy type.
    pub fn get_proxy_type(&self) -> &ProxyType {
        &self.proxy_type
    }

    /// Returns the proxy host.
    pub fn get_host(&self) -> &str {
        &self.host
    }

    /// Returns the proxy port.
    pub fn get_port(&self) -> u16 {
        self.port
    }

    /// Returns the proxy user name.
    pub fn get_user(&self) -> Option<&String> {
        self.user.as_ref()
    }

    /// Returns the proxy password.
    pub fn get_password(&self) -> Option<&String> {
        self.password.as_ref()
    }
}

/// Represents the type of proxy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyType {
    /// HTTP proxy.
    Http,
    /// SOCKS4 proxy.
    Socks4,
    /// SOCKS5 proxy.
    Socks5,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_creation() {
        let proxy = Proxy::new(
            ProxyType::Http,
            "proxy.example.com".to_string(),
            8080,
            Some("username".to_string()),
            Some("password".to_string()),
        );

        assert_eq!(*proxy.get_proxy_type(), ProxyType::Http);
        assert_eq!(proxy.get_host(), "proxy.example.com");
        assert_eq!(proxy.get_port(), 8080);
        assert_eq!(proxy.get_user().unwrap(), "username");
        assert_eq!(proxy.get_password().unwrap(), "password");
    }

    #[test]
    fn test_proxy_creation_without_credentials() {
        let proxy = Proxy::new(
            ProxyType::Socks5,
            "proxy.example.com".to_string(),
            1080,
            None,
            None,
        );

        assert_eq!(*proxy.get_proxy_type(), ProxyType::Socks5);
        assert_eq!(proxy.get_host(), "proxy.example.com");
        assert_eq!(proxy.get_port(), 1080);
        assert_eq!(proxy.get_user(), None);
        assert_eq!(proxy.get_password(), None);
    }

    #[test]
    fn test_proxy_creation_with_username_only() {
        let proxy = Proxy::new(
            ProxyType::Socks4,
            "proxy.example.com".to_string(),
            1080,
            Some("username".to_string()),
            None,
        );

        assert_eq!(*proxy.get_proxy_type(), ProxyType::Socks4);
        assert_eq!(proxy.get_host(), "proxy.example.com");
        assert_eq!(proxy.get_port(), 1080);
        assert_eq!(proxy.get_user().unwrap(), "username");
        assert_eq!(proxy.get_password(), None);
    }

    #[test]
    fn test_proxy_type_equality() {
        assert_eq!(ProxyType::Http, ProxyType::Http);
        assert_eq!(ProxyType::Socks4, ProxyType::Socks4);
        assert_eq!(ProxyType::Socks5, ProxyType::Socks5);
        assert_ne!(ProxyType::Http, ProxyType::Socks4);
        assert_ne!(ProxyType::Http, ProxyType::Socks5);
        assert_ne!(ProxyType::Socks4, ProxyType::Socks5);
    }

    #[test]
    fn test_proxy_debug_format() {
        let proxy = Proxy::new(
            ProxyType::Http,
            "proxy.example.com".to_string(),
            8080,
            Some("username".to_string()),
            Some("password".to_string()),
        );

        let debug_string = format!("{:?}", proxy);

        assert!(debug_string.contains("Http"));
        assert!(debug_string.contains("proxy.example.com"));
        assert!(debug_string.contains("8080"));
        assert!(debug_string.contains("username"));
        assert!(debug_string.contains("password"));
    }

    #[test]
    fn test_proxy_type_debug_format() {
        assert_eq!(format!("{:?}", ProxyType::Http), "Http");
        assert_eq!(format!("{:?}", ProxyType::Socks4), "Socks4");
        assert_eq!(format!("{:?}", ProxyType::Socks5), "Socks5");
    }

    #[test]
    fn test_proxy_with_ipv4_address() {
        // Crear un proxy con dirección IPv4
        let proxy = Proxy::new(
            ProxyType::Http,
            "192.168.1.1".to_string(),
            8080,
            None,
            None,
        );

        assert_eq!(proxy.get_host(), "192.168.1.1");
    }

    #[test]
    fn test_proxy_with_ipv6_address() {
        // Crear un proxy con dirección IPv6
        let proxy = Proxy::new(
            ProxyType::Http,
            "2001:0db8:85a3:0000:0000:8a2e:0370:7334".to_string(),
            8080,
            None,
            None,
        );

        assert_eq!(proxy.get_host(), "2001:0db8:85a3:0000:0000:8a2e:0370:7334");
    }
}