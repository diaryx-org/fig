//! What every language crate fig ships has beside its `register`, written
//! once here and expanded in each (`fig_json`, `fig_yaml`, …), so that
//! `fig_toml::from_str` reads as `toml::from_str` does.

/// Expands, in a language crate whose `register()` is in scope, to
/// `format()`, which registers the language the first time and is a load
/// after, and with the crate's own `serde` feature to `from_str`,
/// `from_slice`, `from_reader`, `to_string`, `to_vec` and `to_writer` in
/// that format. `$lang` is the language's name as its docs spell it.
#[doc(hidden)]
#[macro_export]
macro_rules! __language_crate_functions {
    ($lang:literal) => {
        #[doc = concat!("The format of ", $lang, ", registered the first time it is asked for.")]
        ///
        /// Every function of this crate reads and writes through it, so none
        /// needs a `register()` first. Registration is once per process:
        /// after the first call this is a load.
        pub fn format() -> ::core::result::Result<$crate::Format, $crate::Error> {
            static FORMAT: ::std::sync::OnceLock<$crate::Format> = ::std::sync::OnceLock::new();
            if let ::core::option::Option::Some(format) = FORMAT.get() {
                return ::core::result::Result::Ok(*format);
            }
            let format = register()?;
            ::core::result::Result::Ok(*FORMAT.get_or_init(|| format))
        }

        #[doc = concat!("Deserialize ", $lang, " text into a typed value: [`fig::from_slice`] in [`format()`].")]
        ///
        /// A value that fails to build below the root is
        /// [`fig::Error::At`], which names where.
        #[cfg(feature = "serde")]
        pub fn from_str<T: ::serde::de::DeserializeOwned>(
            s: &str,
        ) -> ::core::result::Result<T, $crate::Error> {
            $crate::from_slice(s.as_bytes(), format()?)
        }

        #[doc = concat!("Deserialize ", $lang, " bytes into a typed value; see [`from_str`].")]
        #[cfg(feature = "serde")]
        pub fn from_slice<T: ::serde::de::DeserializeOwned>(
            bytes: &[u8],
        ) -> ::core::result::Result<T, $crate::Error> {
            $crate::from_slice(bytes, format()?)
        }

        #[doc = concat!("Deserialize ", $lang, " read to its end from `reader` into a typed value; see [`from_str`].")]
        ///
        /// A failed read is [`fig::Error::Io`]. The whole input is read
        /// before any of it is parsed.
        #[cfg(feature = "serde")]
        pub fn from_reader<T: ::serde::de::DeserializeOwned, R: ::std::io::Read>(
            mut reader: R,
        ) -> ::core::result::Result<T, $crate::Error> {
            let mut bytes = ::std::vec::Vec::new();
            reader.read_to_end(&mut bytes)?;
            from_slice(&bytes)
        }

        #[doc = concat!("Serialize a value as ", $lang, " text: [`fig::to_value`], then [`fig::Value::serialize`] in [`format()`].")]
        ///
        /// The text is in fig's default style ([`fig::SerializeOptions::default`]);
        /// for another, serialize the [`fig::Value`] with `serialize_with`.
        #[cfg(feature = "serde")]
        pub fn to_string<T: ::serde::Serialize + ?Sized>(
            value: &T,
        ) -> ::core::result::Result<::std::string::String, $crate::Error> {
            $crate::to_value(value)?.serialize(format()?)
        }

        #[doc = concat!("Serialize a value as ", $lang, " bytes; see [`to_string`].")]
        #[cfg(feature = "serde")]
        pub fn to_vec<T: ::serde::Serialize + ?Sized>(
            value: &T,
        ) -> ::core::result::Result<::std::vec::Vec<u8>, $crate::Error> {
            to_string(value).map(::std::string::String::into_bytes)
        }

        #[doc = concat!("Serialize a value as ", $lang, " into `writer`; see [`to_string`].")]
        ///
        /// A failed write is [`fig::Error::Io`].
        #[cfg(feature = "serde")]
        pub fn to_writer<W: ::std::io::Write, T: ::serde::Serialize + ?Sized>(
            mut writer: W,
            value: &T,
        ) -> ::core::result::Result<(), $crate::Error> {
            writer.write_all(to_string(value)?.as_bytes())?;
            ::core::result::Result::Ok(())
        }
    };
}
