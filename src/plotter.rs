use crate::db::Db;
use anyhow::Error;
use chrono::{NaiveDate, TimeZone, Utc};
use plotters::prelude::*;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::Path;

pub struct Plotter {
    size: (u32, u32),
}

impl Plotter {
    pub fn new() -> Self {
        Plotter { size: (1200, 800) }
    }

    pub fn size(mut self, size: (u32, u32)) -> Self {
        self.size = size;
        self
    }

    pub fn plot<T: AsRef<Path>, U: AsRef<str>>(
        &self,
        path: T,
        targets: &[U],
        db: &Db,
        relative: bool,
        transitive: bool,
        start_date: Option<NaiveDate>,
    ) -> Result<(), Error> {
        let extension = path.as_ref().extension();
        match extension {
            Some(x) if x == OsStr::new("svg") => {
                let backend = SVGBackend::new(path.as_ref(), self.size);
                self.plot_with_backend(backend, targets, db, relative, transitive, start_date)
            }
            _ => {
                let backend = BitMapBackend::new(path.as_ref(), self.size);
                self.plot_with_backend(backend, targets, db, relative, transitive, start_date)
            }
        }
    }

    pub fn plot_with_backend<T, U>(
        &self,
        backend: T,
        targets: &[U],
        db: &Db,
        relative: bool,
        transitive: bool,
        start_date: Option<NaiveDate>,
    ) -> Result<(), Error>
    where
        T: DrawingBackend,
        T::ErrorType: 'static,
        U: AsRef<str>,
    {
        let mut x_min = Utc
            .timestamp_opt(std::i32::MAX as i64, 0)
            .unwrap()
            .date_naive();
        let mut x_max = Utc.timestamp_opt(0, 0).unwrap().date_naive();
        let mut y_min = std::f32::MAX;
        let mut y_max = std::f32::MIN;

        let mut plots = BTreeMap::new();
        for target in targets {
            let mut plot = Vec::new();
            if let Some(entries) = db.map.get(target.as_ref()) {
                for entry in entries {
                    let x_val = entry.time.date_naive();

                    if let Some(start) = start_date {
                        if start > x_val {
                            continue;
                        }
                    }

                    let dependents = if transitive {
                        entry.transitive_dependents
                    } else {
                        entry.direct_dependents
                    };
                    let y_val = if relative {
                        dependents as f32 / entry.total_crates as f32
                    } else {
                        dependents as f32
                    };
                    plot.push((x_val, y_val));

                    x_min = if x_min > x_val { x_val } else { x_min };
                    x_max = if x_max < x_val { x_val } else { x_max };
                    y_min = f32::min(y_min, y_val);
                    y_max = f32::max(y_max, y_val);
                }
            }
            plots.insert(String::from(target.as_ref()), plot);
        }

        y_min *= 0.9;
        y_max *= 1.1;

        let root = backend.into_drawing_area();
        let _ = root.fill(&WHITE);
        let root = root.margin(10, 10, 10, 10);
        let mut chart = ChartBuilder::on(&root)
            .x_label_area_size(50)
            .y_label_area_size(50)
            .build_cartesian_2d(x_min..x_max, y_min..y_max)?;

        let y_desc = if relative {
            "Fraction of dependent crates"
        } else {
            "Number of dependent crates"
        };

        chart
            .configure_mesh()
            .disable_x_mesh()
            .y_label_formatter(&|x| format!("{}", x))
            .y_desc(y_desc)
            .draw()?;

        let hue_step = 1.0 / plots.len() as f64;
        let mut hue = 0.0;
        for (target, plot) in &plots {
            let color = HSLColor(hue, 0.8, 0.5);
            hue += hue_step;

            let style = ShapeStyle {
                color: color.to_rgba(),
                filled: true,
                stroke_width: 2,
            };

            let anno = chart.draw_series(LineSeries::new(plot.clone(), style.clone()))?;
            anno.label(target).legend(move |(x, y)| {
                plotters::prelude::PathElement::new(vec![(x, y), (x + 20, y)], style.clone())
            });
        }

        chart
            .configure_series_labels()
            .position(SeriesLabelPosition::MiddleLeft)
            .background_style(&WHITE)
            .border_style(&BLACK)
            .draw()?;

        chart.plotting_area().present()?;
        Ok(())
    }
}
