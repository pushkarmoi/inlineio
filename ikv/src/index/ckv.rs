use crate::schema::{self, field::Field};

use super::ckv_segment::CKVIndexSegment;
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, BufReader, BufWriter, Error, Read, Write},
    sync::RwLock,
};

const NUM_SEGMENTS: usize = 16;

/// Memmap based columnar key-value index.
pub struct CKVIndex {
    // hash(key) -> PrimaryKeyIndex
    segments: Vec<RwLock<CKVIndexSegment>>,

    // field-id -> Field
    fieldid_field_table: Vec<Field>,

    // field-name -> Field
    fieldname_field_table: HashMap<String, Field>,
}

impl CKVIndex {
    pub fn new(mount_directory: String, schema: &str) -> io::Result<CKVIndex> {
        // ensure mount_directory exists
        fs::create_dir_all(&mount_directory)?;

        // create schema file
        let mut schema_file = BufWriter::new(
            OpenOptions::new()
                .read(true)
                .write(true)
                .truncate(true)
                .create(true)
                .open(format!("{}/schema", mount_directory))?,
        );
        schema_file.write_all(schema.as_bytes())?;

        let mut segments = vec![];
        for index_id in 0..NUM_SEGMENTS {
            let segment = CKVIndexSegment::new(&mount_directory, index_id)?;
            segments.push(RwLock::new(segment));
        }

        let mut fieldid_field_table = schema::load_yaml_schema(schema);
        schema::sort_by_field_id(&mut fieldid_field_table);
        let fieldname_field_table = schema::to_map(&fieldid_field_table);

        Ok(Self {
            segments,
            fieldid_field_table,
            fieldname_field_table,
        })
    }

    pub fn open(mount_directory: String) -> io::Result<CKVIndex> {
        // read schema file
        let schema_file = OpenOptions::new()
            .read(true)
            .write(false)
            .create(false)
            .open(format!("{}/schema", mount_directory))?;
        let mut schema_file = BufReader::new(schema_file);

        let mut schema_str = String::new();
        schema_file
            .read_to_string(&mut schema_str)
            .expect("cant read schema file");

        let mut fieldid_field_table = schema::load_yaml_schema(&schema_str);
        schema::sort_by_field_id(&mut fieldid_field_table);
        let fieldname_field_table = schema::to_map(&fieldid_field_table);

        // open index segments
        let mut segments = Vec::with_capacity(NUM_SEGMENTS);
        for index_id in 0..NUM_SEGMENTS {
            let segment = CKVIndexSegment::open(&mount_directory, index_id)?;
            segments.push(RwLock::new(segment));
        }

        Ok(Self {
            segments,
            fieldid_field_table,
            fieldname_field_table,
        })
    }

    pub fn close(&self) {
        // no op for now
    }

    pub fn get_field_value_by_name(&self, document_id: &[u8], field_name: &str) -> Option<Vec<u8>> {
        let field = self.fieldname_field_table.get(field_name)?;
        self.get_field_value(document_id, field)
    }

    pub fn get_field_value_by_id(&self, document_id: &[u8], field_id: u16) -> Option<Vec<u8>> {
        let field = self.fieldid_field_table.get(field_id as usize)?;
        self.get_field_value(document_id, field)
    }

    fn get_field_value(&self, document_id: &[u8], field: &Field) -> Option<Vec<u8>> {
        let index_id: usize = fxhash::hash(document_id) % NUM_SEGMENTS;
        let primary_key_index: std::sync::RwLockReadGuard<'_, CKVIndexSegment> =
            self.segments[index_id].read().unwrap();
        primary_key_index.read(&document_id, field)
    }

    pub fn upsert_field_value_by_name(
        &self,
        document_id: &[u8],
        field_value: &[u8],
        field_name: &str,
    ) -> io::Result<()> {
        let field = self.fieldname_field_table.get(field_name);
        if field.is_none() {
            return Err(Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid field name",
            ));
        }

        self.upsert_field_value(document_id, field_value, field.unwrap())
    }

    pub fn upsert_field_value_by_id(
        &self,
        document_id: &[u8],
        field_value: &[u8],
        field_id: u16,
    ) -> io::Result<()> {
        let field = self.fieldid_field_table.get(field_id as usize);
        if field.is_none() {
            return Err(Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid field id",
            ));
        }

        self.upsert_field_value(document_id, field_value, field.unwrap())
    }

    fn upsert_field_value(
        &self,
        document_id: &[u8],
        field_value: &[u8],
        field: &Field,
    ) -> io::Result<()> {
        if document_id.len() == 0 {
            return Err(Error::new(
                std::io::ErrorKind::InvalidInput,
                "empty document_id not allowed",
            ));
        }
        if document_id.len() > u16::MAX as usize {
            return Err(Error::new(
                std::io::ErrorKind::Unsupported,
                "document_id larger than 64KB is unsupported",
            ));
        }

        if field_value.len() == 0 {
            return Err(Error::new(
                std::io::ErrorKind::InvalidInput,
                "empty field_value not allowed",
            ));
        }
        if field_value.len() > u32::MAX as usize {
            return Err(Error::new(
                std::io::ErrorKind::Unsupported,
                "value larger than 4GB is unsupported",
            ));
        }

        let index_id = fxhash::hash(document_id) % NUM_SEGMENTS;
        let mut primary_key_index = self.segments[index_id].write().unwrap();
        primary_key_index.upsert(document_id, field_value, field)
    }
}