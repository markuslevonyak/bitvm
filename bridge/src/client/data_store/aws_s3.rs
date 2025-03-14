use crate::{
    error::err_to_string,
    utils::{compress, decompress, DEFAULT_COMPRESSION_LEVEL},
};

use super::base::DataStoreDriver;
use async_trait::async_trait;
use aws_sdk_s3::{
    config::{Credentials, Region},
    error::SdkError,
    operation::put_object::{PutObjectError, PutObjectOutput},
    primitives::ByteStream,
    Client, Config,
};
use dotenv;

// To use this data store, create a .env file in the base directory with the following values:
// export BRIDGE_AWS_ACCESS_KEY_ID="..."
// export BRIDGE_AWS_SECRET_ACCESS_KEY="..."
// export BRIDGE_AWS_REGION="..."
// export BRIDGE_AWS_BUCKET="..."

pub struct AwsS3 {
    client: Client,
    bucket: String,
}

impl AwsS3 {
    pub fn new() -> Option<Self> {
        dotenv::dotenv().ok();
        let access_key = dotenv::var("BRIDGE_AWS_ACCESS_KEY_ID");
        let secret = dotenv::var("BRIDGE_AWS_SECRET_ACCESS_KEY");
        let region = dotenv::var("BRIDGE_AWS_REGION");
        let bucket = dotenv::var("BRIDGE_AWS_BUCKET");

        if access_key.is_err() || secret.is_err() || region.is_err() || bucket.is_err() {
            return None;
        }

        let credentials =
            Credentials::new(access_key.unwrap(), secret.unwrap(), None, None, "Bridge");

        let config = Config::builder()
            .credentials_provider(credentials)
            .region(Region::new(region.unwrap()))
            .behavior_version_latest()
            .build();

        Some(Self {
            client: Client::from_conf(config),
            bucket: bucket.unwrap(),
        })
    }

    async fn get_object(&self, key: &str, file_path: Option<&str>) -> Result<Vec<u8>, String> {
        let key_with_prefix;
        if let Some(path) = file_path {
            key_with_prefix = format! {"{path}/{key}"};
        } else {
            key_with_prefix = key.to_string();
        }

        let mut data = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key_with_prefix)
            .send()
            .await
            .map_err(err_to_string)?;

        let mut buffer: Vec<u8> = vec![];
        while let Some(bytes) = data.body.try_next().await.map_err(err_to_string)? {
            buffer.append(&mut bytes.to_vec());
        }

        Ok(buffer)
    }

    async fn upload_object(
        &self,
        key: &str,
        data: ByteStream,
        file_path: Option<&str>,
    ) -> Result<PutObjectOutput, SdkError<PutObjectError>> {
        let key_with_prefix;
        if let Some(path) = file_path {
            key_with_prefix = format! {"{path}/{key}"};
        } else {
            key_with_prefix = key.to_string();
        }

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key_with_prefix)
            .body(data)
            .send()
            .await
    }
}

#[async_trait]
impl DataStoreDriver for AwsS3 {
    async fn list_objects(&self, file_path: Option<&str>) -> Result<Vec<String>, String> {
        let mut prefix = String::from("");
        if let Some(path) = file_path {
            prefix = format! {"{path}/"};
        }

        let mut response = self
            .client
            .list_objects_v2()
            .prefix(prefix)
            .bucket(&self.bucket)
            .max_keys(50) // Paginate 50 results at a time
            .into_paginator()
            .send();

        let mut keys: Vec<String> = vec![];
        while let Some(result) = response.next().await {
            match result {
                Ok(output) => {
                    for object in output.contents() {
                        keys.push(object.key().unwrap_or("Unknown").to_string());
                    }
                }
                Err(err) => {
                    eprintln!("{err:?}");
                    return Err("Unable to list objects".to_string());
                }
            }
        }

        Ok(keys)
    }

    async fn fetch_object(
        &self,
        file_name: &str,
        file_path: Option<&str>,
    ) -> Result<String, String> {
        let response = self.get_object(file_name, file_path).await;
        match response {
            Ok(buffer) => {
                let json = String::from_utf8(buffer);
                match json {
                    Ok(json) => Ok(json),
                    Err(err) => Err(format!("Failed to parse json: {}", err)),
                }
            }
            Err(err) => Err(format!("Failed to get json file: {}", err)),
        }
    }

    async fn upload_object(
        &self,
        file_name: &str,
        contents: &str,
        file_path: Option<&str>,
    ) -> Result<usize, String> {
        let size = contents.len();
        let byte_stream = ByteStream::from(contents.as_bytes().to_vec());

        match self.upload_object(file_name, byte_stream, file_path).await {
            Ok(_) => Ok(size),
            Err(err) => Err(format!("Failed to save json file: {}", err)),
        }
    }

    async fn fetch_compressed_object(
        &self,
        file_name: &str,
        file_path: Option<&str>,
    ) -> Result<(Vec<u8>, usize), String> {
        let response = self.get_object(file_name, file_path).await;
        match response {
            Ok(buffer) => {
                let size = buffer.len();
                Ok((decompress(&buffer).map_err(err_to_string)?, size))
            }
            Err(err) => Err(format!("Failed to get json file: {}", err)),
        }
    }

    async fn upload_compressed_object(
        &self,
        file_name: &str,
        contents: &Vec<u8>,
        file_path: Option<&str>,
    ) -> Result<usize, String> {
        let compressed_data =
            compress(contents, DEFAULT_COMPRESSION_LEVEL).map_err(err_to_string)?;
        let size = compressed_data.len();
        let byte_stream = ByteStream::from(compressed_data);

        match self.upload_object(file_name, byte_stream, file_path).await {
            Ok(_) => Ok(size),
            Err(err) => Err(format!("Failed to save json file: {}", err)),
        }
    }
}
