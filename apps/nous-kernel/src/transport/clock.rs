// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use nous_core::OperationId;
use nous_runtime::{ClockChange, ClockSnapshot};
fn clock(value: ClockSnapshot) -> p::SubjectClock {
    p::SubjectClock {
        subject_id: value.subject_id.0.to_string(),
        time_domain_ref: value.time_domain_ref.clone(),
        world_anchor: Some(timestamp(value.world_anchor)),
        real_anchor: Some(timestamp(value.real_anchor)),
        rate: value.rate,
        now: Some(p::DomainStamp {
            time_domain_ref: value.time_domain_ref,
            at: Some(timestamp(value.world_now)),
            recorded_at: Some(timestamp(value.recorded_at)),
        }),
        revision: value.revision,
    }
}
#[tonic::async_trait]
impl p::cognitive_clock_service_server::CognitiveClockService for KernelService {
    async fn read_clock(
        &self,
        request: Request<p::SubjectRequest>,
    ) -> std::result::Result<Response<p::SubjectClock>, Status> {
        rpc_reply(async {
            self.0
                .clocks
                .read(SubjectId(id(&request.into_inner().subject_id)?))
                .await
                .map(clock)
        })
        .await
    }
    async fn configure_clock(
        &self,
        request: Request<p::ConfigureClockRequest>,
    ) -> std::result::Result<Response<p::SubjectClock>, Status> {
        rpc_reply(async {
            let input = request.into_inner();
            self.0
                .clocks
                .change(
                    SubjectId(id(&input.subject_id)?),
                    OperationId(id(&input.operation_id)?),
                    ClockChange::Configure {
                        time_domain_ref: input.time_domain_ref,
                        rate: input.rate,
                        world_time: time(input.world_time)?,
                    },
                )
                .await
                .map(clock)
        })
        .await
    }
    async fn pause_clock(
        &self,
        request: Request<p::ClockMutationRequest>,
    ) -> std::result::Result<Response<p::SubjectClock>, Status> {
        rpc_reply(async {
            let input = request.into_inner();
            self.0
                .clocks
                .change(
                    SubjectId(id(&input.subject_id)?),
                    OperationId(id(&input.operation_id)?),
                    ClockChange::Pause,
                )
                .await
                .map(clock)
        })
        .await
    }
    async fn advance_clock(
        &self,
        request: Request<p::AdvanceClockRequest>,
    ) -> std::result::Result<Response<p::SubjectClock>, Status> {
        rpc_reply(async {
            let input = request.into_inner();
            self.0
                .clocks
                .change(
                    SubjectId(id(&input.subject_id)?),
                    OperationId(id(&input.operation_id)?),
                    ClockChange::Advance {
                        seconds: input.seconds,
                    },
                )
                .await
                .map(clock)
        })
        .await
    }
    async fn resume_clock(
        &self,
        request: Request<p::ResumeClockRequest>,
    ) -> std::result::Result<Response<p::SubjectClock>, Status> {
        rpc_reply(async {
            let input = request.into_inner();
            self.0
                .clocks
                .change(
                    SubjectId(id(&input.subject_id)?),
                    OperationId(id(&input.operation_id)?),
                    ClockChange::Resume { rate: input.rate },
                )
                .await
                .map(clock)
        })
        .await
    }
}
