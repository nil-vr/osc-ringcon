use std::{borrow::Cow, ffi::OsString, fs, os::windows::prelude::OsStringExt, path::PathBuf};

use fluent_bundle::{FluentBundle, FluentResource};
use fluent_fallback::{
    generator::{BundleGenerator, FluentBundleResult},
    types::{ResourceId, ResourceType},
    Localization,
};
use fluent_langneg::{negotiate_languages, NegotiationStrategy};
use unic_langid::LanguageIdentifier;
use windows::{core::PWSTR, Win32::Globalization};

pub struct BundleIter {
    locales: std::vec::IntoIter<LanguageIdentifier>,
    res_ids: Vec<ResourceId>,
    path: PathBuf,
}

impl Iterator for BundleIter {
    type Item = FluentBundleResult<FluentResource>;

    fn next(&mut self) -> Option<Self::Item> {
        let locale = self.locales.next()?;
        self.path.clear();
        self.path.push("res");
        self.path.push(locale.to_string());

        let mut bundle = FluentBundle::new(vec![locale]);
        let mut errors = Vec::new();
        for res_id in &self.res_ids {
            self.path.push(&res_id.value);
            let source = match fs::read_to_string(&self.path) {
                Ok(source) => source,
                Err(_) => continue,
            };
            let res = match FluentResource::try_new(source) {
                Ok(res) => res,
                Err((res, err)) => {
                    errors.extend(err.into_iter().map(Into::into));
                    res
                }
            };
            bundle.add_resource(res).unwrap();
        }

        // Disable isolation because it's not supported by iced.
        // iced-rs/iced#33
        bundle.set_use_isolating(false);

        Some(if errors.is_empty() {
            Ok(bundle)
        } else {
            Err((bundle, errors))
        })
    }
}

impl futures::Stream for BundleIter {
    type Item = FluentBundleResult<FluentResource>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        unreachable!()
    }
}

pub struct Bundles;

impl BundleGenerator for Bundles {
    type Resource = FluentResource;
    type LocalesIter = std::vec::IntoIter<LanguageIdentifier>;
    type Iter = BundleIter;
    type Stream = BundleIter;

    fn bundles_iter(&self, locales: Self::LocalesIter, res_ids: Vec<ResourceId>) -> Self::Iter {
        BundleIter {
            locales,
            res_ids,
            path: PathBuf::new(),
        }
    }
}

fn get_locales<I: IntoIterator<Item = T> + Copy, T: AsRef<str>>(
    mut root: PathBuf,
    files: I,
) -> Vec<LanguageIdentifier> {
    unsafe {
        let mut num_languages = 0;
        let mut len = 0;
        if Globalization::GetThreadPreferredUILanguages(
            Globalization::MUI_LANGUAGE_NAME
                | Globalization::MUI_MERGE_SYSTEM_FALLBACK
                | Globalization::MUI_MERGE_USER_FALLBACK,
            &mut num_languages,
            PWSTR::default(),
            &mut len,
        )
        .ok()
        .is_err()
        {
            return Vec::new();
        }
        let mut buffer = Vec::with_capacity(len as usize);
        if Globalization::GetThreadPreferredUILanguages(
            Globalization::MUI_LANGUAGE_NAME
                | Globalization::MUI_MERGE_SYSTEM_FALLBACK
                | Globalization::MUI_MERGE_USER_FALLBACK,
            &mut num_languages,
            PWSTR(buffer.as_mut_ptr()),
            &mut len,
        )
        .ok()
        .is_err()
        {
            return Vec::new();
        }
        buffer.set_len(len as usize);
        let mut locales = Vec::with_capacity(num_languages as usize);

        for locale_wide in buffer.split(|&c| c == 0) {
            let locale_os = OsString::from_wide(locale_wide);
            if let Some(locale) = locale_os.to_str() {
                if let Ok(id) = locale.parse() {
                    if locale.is_empty() {
                        break;
                    }

                    root.push(locale_os);
                    let mut okay = true;
                    'files: for file in files.into_iter() {
                        root.push(file.as_ref());
                        let exists = root.exists();
                        root.pop();
                        if !exists {
                            okay = false;
                            break 'files;
                        }
                    }
                    root.pop();

                    if okay {
                        locales.push(id);
                    }
                }
            }
        }

        locales
    }
}

pub struct Resources {
    localization: Localization<Bundles, Vec<LanguageIdentifier>>,
}

impl Resources {
    pub fn new() -> Self {
        let resource_files = vec![ResourceId::new("main.ftl", ResourceType::Required)];
        let requested = get_locales(
            PathBuf::from("res"),
            &resource_files
                .iter()
                .filter_map(|r| {
                    if r.is_required() {
                        Some(r.value.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>(),
        );
        let fallback = "en-US".parse().unwrap();
        let locales: Vec<_> = negotiate_languages(
            &requested,
            &requested,
            Some(&fallback),
            NegotiationStrategy::Filtering,
        )
        .iter()
        .map(|&id| id.to_owned())
        .collect();
        //let locales = vec!["ja-JP".parse().unwrap(), "ja".parse().unwrap()];

        let localization = Localization::with_env(resource_files, true, locales, Bundles);

        Self { localization }
    }

    pub fn get_string<'a>(&'a self, id: &'a str) -> Cow<'a, str> {
        let mut errors = Vec::new();
        self.localization
            .bundles()
            .format_value_sync(id, None, &mut errors)
            .unwrap()
            .unwrap_or(Cow::Borrowed(""))
    }

    pub fn fonts(&self) -> Vec<String> {
        self.get_string("fonts")
            .split(';')
            .map(|s| s.to_string())
            .collect()
    }

    pub fn bundles(&self) -> &fluent_fallback::Bundles<Bundles> {
        self.localization.bundles()
    }
}
