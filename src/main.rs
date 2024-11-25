use sled;
use lazy_static::lazy_static;

use actix_web::{App, HttpServer, web, Responder, HttpResponse};
use actix_files as fs;
use std::sync::{Arc, Mutex};
use std::collections::{VecDeque, HashMap};
use serde::{Serialize, Deserialize};
use celestia_types::{Commitment, nmt::Namespace};
// The real SP1SDK library is here
//use sp1_sdk::SP1ProofWithPublicValues;
// But we will use an educational mock instead, here:
mod zkproofs;
use zkproofs::{generate_proof, Proof};

use hex;

lazy_static! {
    static ref JOB_DB: sled::Db = sled::open("data/jobs").expect("DB open Error");
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Job {
    commitment: Commitment,
    hash: Option<[u8; 32]>,
    height: u64,
    namespace: Namespace,
    // commented out the Option<SP1ProofWithPublicValues> 
    //replaced it with the mock educational version
    //result: Option<SP1ProofWithPublicValues>,
    result: Option<Proof>,
    status: JobStatus,
}

// Reuqired serde for convinince use in sled
// Could remove serde round trip on DB with  fn
impl From<Job> for sled::IVec {
    fn from(item: Job) -> Self {
        let bytes = bincode::serialize(&item).expect("DB: Job Serialization failed");
        sled::IVec::from(bytes)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum JobStatus {
    InQueue,
    Proving,
    Completed,
    Failed,
}

type CommitmentHash = [u8; 32];

pub struct AppState {
    job_queue: Arc<Mutex<VecDeque<Job>>>,
    job_statuses: sled::Db,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobInfo {
    pub commitment: Commitment,
    pub height: u64,
}

async fn add_job(data: web::Data<AppState>, query: web::Query<HashMap<String, String>>) -> impl Responder {
    
    let height = match query.get("height").and_then(|h| h.parse::<u64>().ok()) {
        Some(h) => h,
        None => return HttpResponse::BadRequest().json("Invalid or missing height parameter"),
    };
    let namespace = match query.get("namespace").and_then(|ns| hex::decode(ns).ok()) {
        Some(ns) => match Namespace::new_v0(&ns) {
            Ok(namespace) => namespace,
            Err(_) => return HttpResponse::BadRequest().json("Couldn't create v0 namespace from provided bytes"),
        },
        _ => return HttpResponse::BadRequest().json("No namespace provided"),
    };
    let commitment = match query.get("commitment").and_then(|c| hex::decode(c).ok()) {
        Some(c) if c.len() == 32 => match c.try_into() {
            Ok(arr) => Commitment(arr),
            Err(_) => return HttpResponse::BadRequest().json("Failed to convert commitment to array"),
        },
        _ => return HttpResponse::BadRequest().json("Invalid commitment parameter"),
    };

    // Check if we have a job for this commitment, if it exists, return the job
    let job_statuses = data.job_statuses.clone();

    if let Ok(Some(raw_job))  = job_statuses.get(&commitment.0) {
        return HttpResponse::Ok().json(format!("{raw_job:?}"));
    }

    // Otherwise, create a job and add it to the back of the queue
    let job = Job {
        commitment,
        height,
        namespace,
        hash: None,
        result: None,
        status: JobStatus::InQueue,
    };
    data.job_queue.lock().unwrap().push_back(job.clone());
    job_statuses.insert(commitment.0, job.clone()).expect("DB: Insert Job Failed");
    HttpResponse::Ok().json(job)
}

async fn get_job(data: web::Data<AppState>, query: web::Query<HashMap<String, String>>) -> impl Responder {
    println!("Getting job");
    let commitment_hash: CommitmentHash = match query.get("commitment").and_then(|c| hex::decode(c).ok()) {
        Some(c) if c.len() == 32 => c.try_into().unwrap(),
        _ => return HttpResponse::BadRequest().json("Invalid commitment hash"),
    };

    let job_statuses = data.job_statuses.clone();
    if let Ok(Some(job)) = job_statuses.get(&commitment_hash) {
        HttpResponse::Ok().json(format!("{job:?}"))
    } else {
        HttpResponse::NotFound().json(format!("Job with commitment hash {} not found", hex::encode(commitment_hash)))
    }
}

fn start_worker(app_state: web::Data<AppState>) {
    println!("Starting worker");
    let state = app_state.clone();
    std::thread::spawn(move || {
        loop {
            let job = {
                let mut queue = state.job_queue.lock().unwrap();
                queue.pop_front()
            };
            // Simulate a job being processed by sleeping, then updating the job status
            if let Some(mut job) = job {
                println!("Processing job: {:?}", job);
                let proof: Proof = generate_proof();
                job.status = JobStatus::Completed;
                job.result = Some(proof);
                println!("Job completed: {:?}", job);

                let job_statuses = state.job_statuses.clone();
                job_statuses.insert(job.commitment.0, job.clone()).expect("DB: Insert Job Failed");
            }
        }
    });
}
    

#[actix_web::main]
async fn main() -> std::io::Result<()> {

    let app_state = web::Data::new(AppState {
        job_queue: Arc::new(Mutex::new(VecDeque::new())),
        job_statuses: JOB_DB.clone(),
    });

    start_worker(app_state.clone());

    HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .route("/add_job", web::get().to(add_job))
            .route("/get_job", web::get().to(get_job))
            .service(fs::Files::new("/", "./static").index_file("index.html"))
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}
