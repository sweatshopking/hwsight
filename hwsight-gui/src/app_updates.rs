use hwsight_core::app_update::{Install, is_newer};

pub enum Banner {
    Hidden,
    Offer { text: String },
    Info { text: String },
    AutoInstall,
}

pub fn banner_for(install: &Install, latest: &str, current: &str) -> Banner {
    if !is_newer(latest, current) {
        return Banner::Hidden;
    }
    match install {
        Install::AppImage(_) => Banner::AutoInstall,
        Install::Package { .. } => Banner::Offer {
            text: format!("hwsight {latest} is available."),
        },
        Install::Aur | Install::Unmanaged => Banner::Info {
            text: format!(
                "hwsight {latest} is available. Update it with your package manager, or download it from the website."
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwsight_core::app_update::{Asset, Install};
    use std::path::PathBuf;

    #[test]
    fn hidden_when_not_newer() {
        assert!(matches!(
            banner_for(&Install::Unmanaged, "0.1.0", "0.1.0"),
            Banner::Hidden
        ));
        assert!(matches!(
            banner_for(&Install::Unmanaged, "0.1.0", "9.9.9"),
            Banner::Hidden
        ));
    }

    #[test]
    fn appimage_installs_automatically() {
        assert!(matches!(
            banner_for(
                &Install::AppImage(PathBuf::from("/tmp/hwsight.AppImage")),
                "9.9.9",
                "0.1.0"
            ),
            Banner::AutoInstall
        ));
    }

    #[test]
    fn packages_get_an_offer() {
        for asset in [Asset::Deb, Asset::Rpm] {
            let banner = banner_for(
                &Install::Package {
                    asset,
                    install: vec![],
                },
                "9.9.9",
                "0.1.0",
            );
            assert!(matches!(
                banner,
                Banner::Offer { text } if text == "hwsight 9.9.9 is available."
            ));
        }
    }

    #[test]
    fn aur_and_source_builds_get_info_only() {
        let expected = "hwsight 9.9.9 is available. Update it with your package manager, or download it from the website.";
        for install in [Install::Aur, Install::Unmanaged] {
            let banner = banner_for(&install, "9.9.9", "0.1.0");
            assert!(matches!(
                banner,
                Banner::Info { text } if text == expected
            ));
        }
    }
}
