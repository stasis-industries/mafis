use csv::WriterBuilder;

use super::data::*;

pub fn to_csv_tables(snapshot: &ExportSnapshot) -> Result<Vec<(String, String)>, String> {
    let tables = vec![
        ("agents".into(), write_agents_csv(&snapshot.agents)?),
        ("faults".into(), write_faults_csv(&snapshot.faults)?),
        ("heatmap".into(), write_heatmap_csv(&snapshot.heatmap)?),
        ("metrics".into(), write_metrics_csv(&snapshot.metrics)?),
    ];

    Ok(tables)
}

fn write_agents_csv(agents: &[ExportAgent]) -> Result<String, String> {
    let mut wtr = WriterBuilder::new().from_writer(Vec::new());
    for agent in agents {
        wtr.serialize(agent).map_err(|e| format!("CSV error: {e}"))?;
    }
    finish_csv(wtr)
}

fn write_faults_csv(faults: &[ExportFault]) -> Result<String, String> {
    let mut wtr = WriterBuilder::new().from_writer(Vec::new());
    for fault in faults {
        wtr.serialize(fault).map_err(|e| format!("CSV error: {e}"))?;
    }
    finish_csv(wtr)
}

fn write_heatmap_csv(cells: &[ExportHeatmapCell]) -> Result<String, String> {
    let mut wtr = WriterBuilder::new().from_writer(Vec::new());
    for cell in cells {
        wtr.serialize(cell).map_err(|e| format!("CSV error: {e}"))?;
    }
    finish_csv(wtr)
}

fn write_metrics_csv(metrics: &ExportMetrics) -> Result<String, String> {
    let mut wtr = WriterBuilder::new().from_writer(Vec::new());
    wtr.serialize(metrics).map_err(|e| format!("CSV error: {e}"))?;
    finish_csv(wtr)
}

fn finish_csv(wtr: csv::Writer<Vec<u8>>) -> Result<String, String> {
    let bytes = wtr.into_inner().map_err(|e| format!("CSV flush error: {e}"))?;
    String::from_utf8(bytes).map_err(|e| format!("CSV UTF-8 error: {e}"))
}
