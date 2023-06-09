use bommer_api::data::{Image, ImageRef, SbomState};
use itertools::Itertools;
use patternfly_yew::prelude::*;
use std::rc::Rc;
use yew::prelude::*;

#[derive(Clone, Debug, PartialEq, Properties)]
pub struct WorkloadTableProperties {
    pub workload: Rc<crate::backend::Workload>,
}

#[derive(PartialEq)]
pub struct WorkloadEntry {
    id: ImageRef,
    state: Image,
}

impl TableEntryRenderer for WorkloadEntry {
    fn render_cell(&self, context: &CellContext) -> Cell {
        match context.column {
            0 => html!(self.id.to_string()).into(),
            1 => html!(self.state.pods.len()).into(),
            2 => match &self.state.sbom {
                SbomState::Scheduled => html!("Retrieving…").into(),
                SbomState::Missing => html!("Missing").into(),
                SbomState::Err(err) => Cell::new(html!(
                    <Tooltip text={err.to_string()}>
                        { format!("Failed ({err})") }
                    </Tooltip>
                ))
                .text_modifier(TextModifier::Truncate),
                SbomState::Found(_) => html!("Found").into(),
            },
            _ => Default::default(),
        }
        .into()
    }

    fn render_details(&self) -> Vec<Span> {
        vec![Span::max(html!(
            <ul>
                { for self.state.pods.iter().sorted_unstable().map(| pod|{
                    html!(<li> { &pod.namespace }  { " / " } { &pod.name} </li> )
                })}
            </ul>
        ))]
    }
}

#[function_component(WorkloadTable)]
pub fn workload_table(props: &WorkloadTableProperties) -> Html {
    let header = html_nested!(
        <TableHeader>
            <TableColumn label="Image" width={ColumnWidth::Percent(80)} />
            <TableColumn label="Pods" width={ColumnWidth::Percent(5)}   />
            <TableColumn label="SBOM" width={ColumnWidth::Percent(10)}  />
        </TableHeader>
    );

    let entries = use_memo(
        |workload| {
            let mut entries = SharedTableModel::with_capacity(workload.0.len());
            for (k, v) in workload.0.iter().sorted_unstable_by_key(|(k, _)| *k) {
                entries.push(WorkloadEntry {
                    id: k.clone(),
                    state: v.clone(),
                })
            }
            entries
        },
        props.workload.clone(),
    );

    html!(
        <Table<SharedTableModel<WorkloadEntry>>
            {header}
            grid={TableGridMode::Medium}
            entries={(*entries).clone()}
            mode={TableMode::CompactExpandable}
        />
    )
}
