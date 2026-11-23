mod bevy_viewport;
mod logs_panel;
mod sky_search;
mod star_details;
mod theme_loader;
mod theme_provider;
mod tracing;

use crate::tracing as apogee_tracing;

use assets::Assets;
use gpui::{App, AppContext, WindowOptions};
use gpui_platform::application;
use workspace::workbench::Workbench;


fn main() {
    let app_start = std::time::Instant::now();
    apogee_tracing::init_tracing();

    // Looks like tracy hardcods "MainThread" for the main thread's name
    // Renaming here only leads to helper threads (mainly for zbus) inheriting
    // this name, which is confusing in the tracy client
    //
    // #[cfg(all(debug_assertions, feature = "tracy"))]
    // if let Some(client) = tracy_client::Client::running() {
    //     client.set_thread_name("GPUI");
    // }

    let startup_span = ::tracing::info_span!("startup").entered();

    let runtime = {
        let _span = ::tracing::info_span!("tokio_runtime_init").entered();
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to initialize Tokio runtime")
    };
    let _runtime_guard = runtime.enter();

    let app_builder = {
        let _span = ::tracing::info_span!("gpui_application_create").entered();
        application().with_assets(Assets)
    };

    let _run_span = ::tracing::info_span!("gpui_run_loop_enter").entered();
    app_builder.run(move |cx: &mut App| {
        {
            let _span = ::tracing::info_span!("load_fonts").entered();
            Assets.load_fonts(cx).expect("failed to load fonts");
        }
        {
            let _span = ::tracing::info_span!("theme_init").entered();
            theme::init(theme::LoadThemes::All(Box::new(Assets)), cx);
        }
        {
            let _span = ::tracing::info_span!("theme_provider_init").entered();
            theme_provider::init(cx);
        }

        {
            let _span = ::tracing::info_span!("settings_init").entered();
            settings::init(cx);
        }

        {
            let _span = ::tracing::info_span!("horizon_init").entered();
            horizon::init(cx);
        }

        {
            let _span = ::tracing::info_span!("observer_init").entered();
            observer::init(cx);
        }

        {
            let _span = ::tracing::info_span!("client_init").entered();
            client::init(cx);
        }

        {
            let _span = ::tracing::info_span!("status_bar_init").entered();
            status_bar::init(cx);
        }
        {
            let _span = ::tracing::info_span!("title_bar_init").entered();
            title_bar::init(cx);
        }
        {
            let _span = ::tracing::info_span!("star_details_init").entered();
            star_details::init(cx);
        }
        {
            let _span = ::tracing::info_span!("sky_search_init").entered();
            sky_search::init(cx);
        }
        {
            let _span = ::tracing::info_span!("bevy_viewport_init").entered();
            bevy_viewport::init(cx);
        }
        {
            let _span = ::tracing::info_span!("logs_panel_init").entered();
            logs_panel::init(cx);
        }
        {
            let _span = ::tracing::info_span!("raw_viewer_init").entered();
            raw_viewer::init(cx);
        }


        drop(startup_span);

        let open_window_res = {
            let _span = ::tracing::info_span!("open_main_window").entered();
            cx.open_window(WindowOptions::default(), |window, cx| {
                let _wb_span = ::tracing::info_span!("workbench_new").entered();
                cx.new(|cx| Workbench::new(window, cx))
            })
        };

        match open_window_res {
            Ok(_) => {
                ::tracing::info!(
                    "Main window opened at +{:.2}ms from app start",
                    app_start.elapsed().as_secs_f64() * 1000.0
                );
            }
            Err(e) => {
                ::tracing::error!("Failed to open main window: {e:?}");
            }
        }
    });
}
