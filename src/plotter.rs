use crate::db::Db;
use chrono::{TimeZone, Utc};
use failure::Error;
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
    ) -> Result<(), Error> {
        let extension = path.as_ref().extension();
        match extension {
            Some(x) if x == OsStr::new("svg") => {
                let backend = SVGBackend::new(path.as_ref(), self.size);
                self.plot_with_backend(backend, targets, db)
            }
            _ => {
                let backend = BitMapBackend::new(path.as_ref(), self.size);
                self.plot_with_backend(backend, targets, db)
            }
        }
    }

    pub fn plot_with_backend<T, U>(&self, backend: T, targets: &[U], db: &Db) -> Result<(), Error>
    where
        T: DrawingBackend,
        T::ErrorType: 'static,
        U: AsRef<str>,
    {
        let mut x_min = Utc.timestamp(std::i32::MAX as i64, 0).date();
        let mut x_max = Utc.timestamp(0, 0).date();
        let mut y_min = std::f32::MAX;
        let mut y_max = std::f32::MIN;

        let mut plots = BTreeMap::new();
        for target in targets {
            let mut plot = Vec::new();
            if let Some(entries) = db.map.get(target.as_ref()) {
                for entry in entries {
                    let date = entry.time.date();
                    let dependents = entry.dependents as f32;
                    plot.push((date, dependents));

                    x_min = if x_min > date { date } else { x_min };
                    x_max = if x_max < date { date } else { x_max };
                    y_min = f32::min(y_min, dependents);
                    y_max = f32::max(y_max, dependents);
                }
            }
            plots.insert(String::from(target.as_ref()), plot);
        }

        let root = backend.into_drawing_area();
        let _ = root.fill(&WHITE);
        let root = root.margin(10, 10, 10, 10);
        let mut chart = ChartBuilder::on(&root)
            .x_label_area_size(50)
            .y_label_area_size(50)
            .build_ranged(x_min..x_max, y_min..y_max)?;

        chart
            .configure_mesh()
            .disable_x_mesh()
            .y_label_formatter(&|x| format!("{:.0}", x))
            .y_desc("Number of dependent crates")
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

        let _ = chart
            .configure_series_labels()
            .background_style(&WHITE)
            .border_style(&BLACK)
            .draw();

        Ok(())
    }
}
