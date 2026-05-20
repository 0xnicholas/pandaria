macro_rules! define_media_provider {
    (
        $struct_name:ident,
        $provider_str:literal,
        $env_key:literal,
        $default_url:literal
    ) => {
        pub struct $struct_name {
            config: crate::providers::shared::ProviderConfig,
        }

        impl $struct_name {
            pub fn new(api_key: Option<secrecy::SecretString>) -> Self {
                Self::with_base_url(api_key, $default_url)
            }

            pub fn with_base_url(
                api_key: Option<secrecy::SecretString>,
                base_url: &str,
            ) -> Self {
                Self {
                    config: crate::providers::shared::ProviderConfig::new(
                        api_key,
                        base_url,
                        $provider_str,
                        $env_key,
                    ),
                }
            }

            pub fn with_client(
                client: reqwest::Client,
                api_key: Option<secrecy::SecretString>,
                base_url: &str,
            ) -> Self {
                Self {
                    config: crate::providers::shared::ProviderConfig::with_client(
                        client,
                        api_key,
                        base_url,
                        $provider_str,
                        $env_key,
                    ),
                }
            }

            pub fn config(&self) -> &crate::providers::shared::ProviderConfig {
                &self.config
            }
        }
    };
}

pub(crate) use define_media_provider;
