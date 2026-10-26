use super::Entry;
use nous_domain::{media::DerivedRepresentation, *};

struct MediaTimes {
    event: Option<TimePoint>,
    observation: Option<TimePoint>,
    formation: Option<TimePoint>,
    recorded: TimePoint,
}
impl MediaTimes {
    fn point(&self, axis: &TimeAxis) -> Option<&TimePoint> {
        match axis {
            TimeAxis::Event => self.event.as_ref(),
            TimeAxis::Observation => self.observation.as_ref(),
            TimeAxis::Formation => self.formation.as_ref(),
            TimeAxis::Recorded => Some(&self.recorded),
            TimeAxis::Validity => None,
        }
    }
    fn eligible(&self, request: &QueryRequest) -> bool {
        filters(request).all(|filter| {
            self.point(&filter.axis)
                .is_some_and(|point| filter.range.contains(point))
        })
    }
}
impl Entry {
    pub(super) fn matches_times(&self, request: &QueryRequest) -> bool {
        if let Some(times) = self.object.cognitive_times() {
            return filters(request).all(|filter| filter.matches(times));
        }
        if filters(request).next().is_none() {
            return true;
        }
        self.media_times()
            .iter()
            .any(|group| group.eligible(request))
    }
    pub fn sort_time(&self, axis: &TimeAxis, request: &QueryRequest) -> Option<TimePoint> {
        let mut points = if let Some(times) = self.object.cognitive_times() {
            if !filters(request).all(|filter| filter.matches(times)) {
                return None;
            }
            let point = match axis {
                TimeAxis::Event => times.event.clone(),
                TimeAxis::Observation => times.observation.clone(),
                TimeAxis::Formation => Some(times.formation.clone()),
                TimeAxis::Recorded => Some(times.recorded.clone()),
                TimeAxis::Validity => times.validity.as_ref().and_then(|range| {
                    range.start.map(|coordinate| TimePoint {
                        domain: range.domain.clone(),
                        coordinate,
                    })
                }),
            };
            point.into_iter().collect::<Vec<_>>()
        } else {
            self.media_times()
                .into_iter()
                .filter(|group| group.eligible(request))
                .filter_map(|group| group.point(axis).cloned())
                .collect()
        };
        let domain = request
            .sort
            .as_ref()
            .and_then(|sort| sort.domain.as_deref())
            .or_else(|| {
                filters(request)
                    .find(|filter| filter.axis == *axis)
                    .map(|filter| filter.range.domain.as_str())
            });
        if let Some(domain) = domain {
            points.retain(|point| point.domain == domain);
        } else if let Some(first) = points.first()
            && points.iter().any(|point| point.domain != first.domain)
        {
            return None;
        }
        if request
            .sort
            .as_ref()
            .is_some_and(|sort| matches!(sort.direction, SortDirection::Asc))
        {
            points.into_iter().min_by_key(|point| point.coordinate)
        } else {
            points.into_iter().max_by_key(|point| point.coordinate)
        }
    }
    fn media_times(&self) -> Vec<MediaTimes> {
        let declared = self.declared_representation();
        let actual = declared.and_then(|value| value.observation_id.as_ref());
        let formation = declared.map(|value| value.formed_at.clone());
        let mut groups = Vec::new();
        for root in &self.materials {
            let observations = self
                .observations
                .iter()
                .filter(|observation| {
                    observation.material_id == root.material.id
                        && actual.is_none_or(|actual| observation.id == *actual)
                })
                .collect::<Vec<_>>();
            if observations.is_empty() {
                if actual.is_none() {
                    groups.push(MediaTimes {
                        event: root.material.event_time.clone(),
                        observation: None,
                        formation: formation.clone(),
                        recorded: self.recorded.clone(),
                    });
                }
            } else {
                for observation in observations {
                    groups.push(MediaTimes {
                        event: root.material.event_time.clone(),
                        observation: observation.observed_at.clone(),
                        formation: formation.clone(),
                        recorded: self.recorded.clone(),
                    });
                }
            }
        }
        groups
    }
    fn declared_representation(&self) -> Option<&DerivedRepresentation> {
        let reference = match &self.object {
            RetrievalRecord::Media(MediaRecord::Representation(value)) => value.reference(),
            RetrievalRecord::Media(MediaRecord::DerivedRegion(value)) => {
                value.representation.clone()
            }
            _ => return None,
        };
        self.media_source
            .as_ref()?
            .representations
            .iter()
            .find(|value| value.reference() == reference)
    }
}
fn filters(request: &QueryRequest) -> impl Iterator<Item = &TimeFilter> {
    request.time.iter().chain(&request.times)
}
