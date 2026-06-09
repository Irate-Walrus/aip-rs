// @generated
/// Generated server implementations.
pub mod freight_service_server {
    #![allow(
        unused_variables,
        dead_code,
        missing_docs,
        clippy::wildcard_imports,
        clippy::let_unit_value,
    )]
    use tonic::codegen::*;
    /// Generated trait containing gRPC methods that should be implemented for use with FreightServiceServer.
    #[async_trait]
    pub trait FreightService: std::marker::Send + std::marker::Sync + 'static {
        async fn get_shipper(
            &self,
            request: tonic::Request<super::GetShipperRequest>,
        ) -> std::result::Result<tonic::Response<super::Shipper>, tonic::Status>;
        async fn list_shippers(
            &self,
            request: tonic::Request<super::ListShippersRequest>,
        ) -> std::result::Result<
            tonic::Response<super::ListShippersResponse>,
            tonic::Status,
        >;
        async fn create_shipper(
            &self,
            request: tonic::Request<super::CreateShipperRequest>,
        ) -> std::result::Result<tonic::Response<super::Shipper>, tonic::Status>;
        async fn update_shipper(
            &self,
            request: tonic::Request<super::UpdateShipperRequest>,
        ) -> std::result::Result<tonic::Response<super::Shipper>, tonic::Status>;
        async fn delete_shipper(
            &self,
            request: tonic::Request<super::DeleteShipperRequest>,
        ) -> std::result::Result<tonic::Response<super::Shipper>, tonic::Status>;
        async fn get_site(
            &self,
            request: tonic::Request<super::GetSiteRequest>,
        ) -> std::result::Result<tonic::Response<super::Site>, tonic::Status>;
        async fn list_sites(
            &self,
            request: tonic::Request<super::ListSitesRequest>,
        ) -> std::result::Result<
            tonic::Response<super::ListSitesResponse>,
            tonic::Status,
        >;
        async fn create_site(
            &self,
            request: tonic::Request<super::CreateSiteRequest>,
        ) -> std::result::Result<tonic::Response<super::Site>, tonic::Status>;
        async fn update_site(
            &self,
            request: tonic::Request<super::UpdateSiteRequest>,
        ) -> std::result::Result<tonic::Response<super::Site>, tonic::Status>;
        async fn delete_site(
            &self,
            request: tonic::Request<super::DeleteSiteRequest>,
        ) -> std::result::Result<tonic::Response<super::Site>, tonic::Status>;
        async fn batch_get_sites(
            &self,
            request: tonic::Request<super::BatchGetSitesRequest>,
        ) -> std::result::Result<
            tonic::Response<super::BatchGetSitesResponse>,
            tonic::Status,
        >;
        async fn get_shipment(
            &self,
            request: tonic::Request<super::GetShipmentRequest>,
        ) -> std::result::Result<tonic::Response<super::Shipment>, tonic::Status>;
        async fn list_shipments(
            &self,
            request: tonic::Request<super::ListShipmentsRequest>,
        ) -> std::result::Result<
            tonic::Response<super::ListShipmentsResponse>,
            tonic::Status,
        >;
        async fn create_shipment(
            &self,
            request: tonic::Request<super::CreateShipmentRequest>,
        ) -> std::result::Result<tonic::Response<super::Shipment>, tonic::Status>;
        async fn update_shipment(
            &self,
            request: tonic::Request<super::UpdateShipmentRequest>,
        ) -> std::result::Result<tonic::Response<super::Shipment>, tonic::Status>;
        async fn delete_shipment(
            &self,
            request: tonic::Request<super::DeleteShipmentRequest>,
        ) -> std::result::Result<tonic::Response<super::Shipment>, tonic::Status>;
    }
    #[derive(Debug)]
    pub struct FreightServiceServer<T> {
        inner: Arc<T>,
        accept_compression_encodings: EnabledCompressionEncodings,
        send_compression_encodings: EnabledCompressionEncodings,
        max_decoding_message_size: Option<usize>,
        max_encoding_message_size: Option<usize>,
    }
    impl<T> FreightServiceServer<T> {
        pub fn new(inner: T) -> Self {
            Self::from_arc(Arc::new(inner))
        }
        pub fn from_arc(inner: Arc<T>) -> Self {
            Self {
                inner,
                accept_compression_encodings: Default::default(),
                send_compression_encodings: Default::default(),
                max_decoding_message_size: None,
                max_encoding_message_size: None,
            }
        }
        pub fn with_interceptor<F>(
            inner: T,
            interceptor: F,
        ) -> InterceptedService<Self, F>
        where
            F: tonic::service::Interceptor,
        {
            InterceptedService::new(Self::new(inner), interceptor)
        }
        /// Enable decompressing requests with the given encoding.
        #[must_use]
        pub fn accept_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.accept_compression_encodings.enable(encoding);
            self
        }
        /// Compress responses with the given encoding, if the client supports it.
        #[must_use]
        pub fn send_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.send_compression_encodings.enable(encoding);
            self
        }
        /// Limits the maximum size of a decoded message.
        ///
        /// Default: `4MB`
        #[must_use]
        pub fn max_decoding_message_size(mut self, limit: usize) -> Self {
            self.max_decoding_message_size = Some(limit);
            self
        }
        /// Limits the maximum size of an encoded message.
        ///
        /// Default: `usize::MAX`
        #[must_use]
        pub fn max_encoding_message_size(mut self, limit: usize) -> Self {
            self.max_encoding_message_size = Some(limit);
            self
        }
    }
    impl<T, B> tonic::codegen::Service<http::Request<B>> for FreightServiceServer<T>
    where
        T: FreightService,
        B: Body + std::marker::Send + 'static,
        B::Error: Into<StdError> + std::marker::Send + 'static,
    {
        type Response = http::Response<tonic::body::Body>;
        type Error = std::convert::Infallible;
        type Future = BoxFuture<Self::Response, Self::Error>;
        fn poll_ready(
            &mut self,
            _cx: &mut Context<'_>,
        ) -> Poll<std::result::Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, req: http::Request<B>) -> Self::Future {
            match req.uri().path() {
                "/einride.example.freight.v1.FreightService/GetShipper" => {
                    #[allow(non_camel_case_types)]
                    struct GetShipperSvc<T: FreightService>(pub Arc<T>);
                    impl<
                        T: FreightService,
                    > tonic::server::UnaryService<super::GetShipperRequest>
                    for GetShipperSvc<T> {
                        type Response = super::Shipper;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::GetShipperRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as FreightService>::get_shipper(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = GetShipperSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/einride.example.freight.v1.FreightService/ListShippers" => {
                    #[allow(non_camel_case_types)]
                    struct ListShippersSvc<T: FreightService>(pub Arc<T>);
                    impl<
                        T: FreightService,
                    > tonic::server::UnaryService<super::ListShippersRequest>
                    for ListShippersSvc<T> {
                        type Response = super::ListShippersResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::ListShippersRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as FreightService>::list_shippers(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = ListShippersSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/einride.example.freight.v1.FreightService/CreateShipper" => {
                    #[allow(non_camel_case_types)]
                    struct CreateShipperSvc<T: FreightService>(pub Arc<T>);
                    impl<
                        T: FreightService,
                    > tonic::server::UnaryService<super::CreateShipperRequest>
                    for CreateShipperSvc<T> {
                        type Response = super::Shipper;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::CreateShipperRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as FreightService>::create_shipper(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = CreateShipperSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/einride.example.freight.v1.FreightService/UpdateShipper" => {
                    #[allow(non_camel_case_types)]
                    struct UpdateShipperSvc<T: FreightService>(pub Arc<T>);
                    impl<
                        T: FreightService,
                    > tonic::server::UnaryService<super::UpdateShipperRequest>
                    for UpdateShipperSvc<T> {
                        type Response = super::Shipper;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::UpdateShipperRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as FreightService>::update_shipper(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = UpdateShipperSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/einride.example.freight.v1.FreightService/DeleteShipper" => {
                    #[allow(non_camel_case_types)]
                    struct DeleteShipperSvc<T: FreightService>(pub Arc<T>);
                    impl<
                        T: FreightService,
                    > tonic::server::UnaryService<super::DeleteShipperRequest>
                    for DeleteShipperSvc<T> {
                        type Response = super::Shipper;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::DeleteShipperRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as FreightService>::delete_shipper(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = DeleteShipperSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/einride.example.freight.v1.FreightService/GetSite" => {
                    #[allow(non_camel_case_types)]
                    struct GetSiteSvc<T: FreightService>(pub Arc<T>);
                    impl<
                        T: FreightService,
                    > tonic::server::UnaryService<super::GetSiteRequest>
                    for GetSiteSvc<T> {
                        type Response = super::Site;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::GetSiteRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as FreightService>::get_site(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = GetSiteSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/einride.example.freight.v1.FreightService/ListSites" => {
                    #[allow(non_camel_case_types)]
                    struct ListSitesSvc<T: FreightService>(pub Arc<T>);
                    impl<
                        T: FreightService,
                    > tonic::server::UnaryService<super::ListSitesRequest>
                    for ListSitesSvc<T> {
                        type Response = super::ListSitesResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::ListSitesRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as FreightService>::list_sites(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = ListSitesSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/einride.example.freight.v1.FreightService/CreateSite" => {
                    #[allow(non_camel_case_types)]
                    struct CreateSiteSvc<T: FreightService>(pub Arc<T>);
                    impl<
                        T: FreightService,
                    > tonic::server::UnaryService<super::CreateSiteRequest>
                    for CreateSiteSvc<T> {
                        type Response = super::Site;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::CreateSiteRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as FreightService>::create_site(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = CreateSiteSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/einride.example.freight.v1.FreightService/UpdateSite" => {
                    #[allow(non_camel_case_types)]
                    struct UpdateSiteSvc<T: FreightService>(pub Arc<T>);
                    impl<
                        T: FreightService,
                    > tonic::server::UnaryService<super::UpdateSiteRequest>
                    for UpdateSiteSvc<T> {
                        type Response = super::Site;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::UpdateSiteRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as FreightService>::update_site(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = UpdateSiteSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/einride.example.freight.v1.FreightService/DeleteSite" => {
                    #[allow(non_camel_case_types)]
                    struct DeleteSiteSvc<T: FreightService>(pub Arc<T>);
                    impl<
                        T: FreightService,
                    > tonic::server::UnaryService<super::DeleteSiteRequest>
                    for DeleteSiteSvc<T> {
                        type Response = super::Site;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::DeleteSiteRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as FreightService>::delete_site(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = DeleteSiteSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/einride.example.freight.v1.FreightService/BatchGetSites" => {
                    #[allow(non_camel_case_types)]
                    struct BatchGetSitesSvc<T: FreightService>(pub Arc<T>);
                    impl<
                        T: FreightService,
                    > tonic::server::UnaryService<super::BatchGetSitesRequest>
                    for BatchGetSitesSvc<T> {
                        type Response = super::BatchGetSitesResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::BatchGetSitesRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as FreightService>::batch_get_sites(&inner, request)
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = BatchGetSitesSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/einride.example.freight.v1.FreightService/GetShipment" => {
                    #[allow(non_camel_case_types)]
                    struct GetShipmentSvc<T: FreightService>(pub Arc<T>);
                    impl<
                        T: FreightService,
                    > tonic::server::UnaryService<super::GetShipmentRequest>
                    for GetShipmentSvc<T> {
                        type Response = super::Shipment;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::GetShipmentRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as FreightService>::get_shipment(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = GetShipmentSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/einride.example.freight.v1.FreightService/ListShipments" => {
                    #[allow(non_camel_case_types)]
                    struct ListShipmentsSvc<T: FreightService>(pub Arc<T>);
                    impl<
                        T: FreightService,
                    > tonic::server::UnaryService<super::ListShipmentsRequest>
                    for ListShipmentsSvc<T> {
                        type Response = super::ListShipmentsResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::ListShipmentsRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as FreightService>::list_shipments(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = ListShipmentsSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/einride.example.freight.v1.FreightService/CreateShipment" => {
                    #[allow(non_camel_case_types)]
                    struct CreateShipmentSvc<T: FreightService>(pub Arc<T>);
                    impl<
                        T: FreightService,
                    > tonic::server::UnaryService<super::CreateShipmentRequest>
                    for CreateShipmentSvc<T> {
                        type Response = super::Shipment;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::CreateShipmentRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as FreightService>::create_shipment(&inner, request)
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = CreateShipmentSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/einride.example.freight.v1.FreightService/UpdateShipment" => {
                    #[allow(non_camel_case_types)]
                    struct UpdateShipmentSvc<T: FreightService>(pub Arc<T>);
                    impl<
                        T: FreightService,
                    > tonic::server::UnaryService<super::UpdateShipmentRequest>
                    for UpdateShipmentSvc<T> {
                        type Response = super::Shipment;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::UpdateShipmentRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as FreightService>::update_shipment(&inner, request)
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = UpdateShipmentSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/einride.example.freight.v1.FreightService/DeleteShipment" => {
                    #[allow(non_camel_case_types)]
                    struct DeleteShipmentSvc<T: FreightService>(pub Arc<T>);
                    impl<
                        T: FreightService,
                    > tonic::server::UnaryService<super::DeleteShipmentRequest>
                    for DeleteShipmentSvc<T> {
                        type Response = super::Shipment;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::DeleteShipmentRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as FreightService>::delete_shipment(&inner, request)
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = DeleteShipmentSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                _ => {
                    Box::pin(async move {
                        let mut response = http::Response::new(
                            tonic::body::Body::default(),
                        );
                        let headers = response.headers_mut();
                        headers
                            .insert(
                                tonic::Status::GRPC_STATUS,
                                (tonic::Code::Unimplemented as i32).into(),
                            );
                        headers
                            .insert(
                                http::header::CONTENT_TYPE,
                                tonic::metadata::GRPC_CONTENT_TYPE,
                            );
                        Ok(response)
                    })
                }
            }
        }
    }
    impl<T> Clone for FreightServiceServer<T> {
        fn clone(&self) -> Self {
            let inner = self.inner.clone();
            Self {
                inner,
                accept_compression_encodings: self.accept_compression_encodings,
                send_compression_encodings: self.send_compression_encodings,
                max_decoding_message_size: self.max_decoding_message_size,
                max_encoding_message_size: self.max_encoding_message_size,
            }
        }
    }
    /// Generated gRPC service name
    pub const SERVICE_NAME: &str = "einride.example.freight.v1.FreightService";
    impl<T> tonic::server::NamedService for FreightServiceServer<T> {
        const NAME: &'static str = SERVICE_NAME;
    }
}
