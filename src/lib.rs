#![feature(c_unwind, once_cell, slice_pattern)]
#![allow(dead_code, unused_assignments, unused_variables)] //debug only, will be removed in production

//test commands
//lua_run_cl require("goqui") PrintTable(goqui.GetModels())
//lua_run_cl print(goqui.Compute("goqui/gordead_ans18.wav", "en-us", function(...) print("callback!", ...) end))

use byteorder::{LittleEndian, ReadBytesExt};
use dasp_signal::Signal;

use gmod::{
	lua::{State as GLuaState, LuaFunction, LuaString},
	gmcl::override_stdout
};

use core::slice::SlicePattern;
use std::{
	collections::HashMap,
	fs::{File, ReadDir},
	net::UdpSocket,
	path::{Path, PathBuf},
	sync::{LazyLock, Mutex},
	thread
};

#[macro_use]
extern crate gmod;

//constants
const VOICE_BUFFER_SIZE: usize = 636;
const VOICE_COLLECTION_THRESHOLD: usize = 16;
const VOICE_HEADED_SIZE: usize = VOICE_BUFFER_SIZE + 4;
const VOICE_HEADED_64: u64 = VOICE_HEADED_SIZE as u64;
const VOICE_SAMPLE_RATE: u32 = 24000;
const VOICE_REPACKED_SIZE: usize = VOICE_BUFFER_SIZE * VOICE_COLLECTION_THRESHOLD;

//types
type E<'a> = core::result::Result<(), &'a str>;
type VoiceBuilder = HashMap<u64, Vec<[u8; VOICE_BUFFER_SIZE]>>;
type VoiceMerger = [[u8; VOICE_BUFFER_SIZE]; VOICE_COLLECTION_THRESHOLD];

//structs
struct LoadedCoquiModel {
	model: coqui_stt::Model,
	model_name: Box<Path>,
	name: String,
	sample_rate: u32,
	scorer_enabled: bool,
	scorer_name: Option<Box<Path>>,
}

//implementations
impl LoadedCoquiModel {
	fn clone(&self) -> Result<Self, &'static str> {
		let model_name = self.model_name.clone();
		let scorer_name = self.scorer_name.clone();
		
		let Ok(mut model) = coqui_stt::Model::new(model_name.to_str().unwrap()) else {return Err("Coqui failed to load the model")};
		let mut scorer_enabled = false;
		if let Some(scorer) = scorer_name {if model.enable_external_scorer(scorer.to_str().unwrap()).is_ok() {scorer_enabled = true}}
		
		Ok(LoadedCoquiModel {
			model_name,
			name: self.name.clone(),
			sample_rate: model.get_sample_rate() as u32,
			scorer_enabled,
			scorer_name: self.scorer_name.clone(),
			
			//we have to set this after sample_rate since we're moving it
			model,
		})
	}
}

//statics
static mut DATA_DIRECTORY: String = String::new();
static GMOD_PATH: LazyLock<PathBuf> = LazyLock::new(|| std::env::current_dir().expect("failed to access working directory"));
static mut VOICE_QUEUE: std::sync::Mutex<Vec<String>> = Mutex::new(vec![]);

static MODEL_TABLE: LazyLock<HashMap<String, LoadedCoquiModel>> = LazyLock::new(|| {
	let mut table = HashMap::new();
	
	println!("[Goqui] Loading models...");
	
	//called for each directory in our models folder
	fn load_model(directory_path: PathBuf) -> Result<LoadedCoquiModel, &'static str> {
		let Ok(directory_iterator) = directory_path.read_dir() else {return Err("failed to access model directory")};
		
		let mut model_name: Option<Box<Path>> = None;
		let mut scorer_name: Option<Box<Path>> = None;
		
		for file in directory_iterator.flatten() {
			let file_path = file.path();
			
			if file_path.is_file() {
				if let Some(extension) = file_path.extension() {
					if extension == "tflite" {model_name = Some(file_path.into_boxed_path())}
					else if extension == "scorer" {scorer_name = Some(file_path.into_boxed_path())}
				}
			}
		}
		
		let Some(model_name) = model_name else {return Err("missing a .tflite model file")};
		let Ok(mut model) = coqui_stt::Model::new(model_name.to_str().unwrap()) else {return Err("Coqui failed to load the model")};
		let mut scorer_enabled = false;
		if let Some(scorer) = scorer_name.clone() {if model.enable_external_scorer(scorer.to_str().unwrap()).is_ok() {scorer_enabled = true}}
		
		Ok(LoadedCoquiModel {
			model_name,
			name: String::new(),
			sample_rate: model.get_sample_rate() as u32,
			scorer_enabled,
			scorer_name,
			
			//we have to set this after sample_rate since we're moving it
			model,
		})
	}
	
	//for easier error handling
	let directory_iterator = || -> Result<ReadDir, &str> {
		//one day, this won't be unsafe :D
		//unsafe {DATA_DIRECTORY += &format!("{}/garrysmod/data/", gmod_exe_path.to_str().unwrap())}
		//DATA_DIRECTORY
		
		let gmod_path = GMOD_PATH.to_str().unwrap();
		let mut gmod_bin = GMOD_PATH.clone();
		
		unsafe {DATA_DIRECTORY = format!("{gmod_path}/garrysmod/data/")}
		
		gmod_bin.push("garrysmod/lua/bin/goqui");
		
		//give up if the directory doesn't exist and we fail to create it
		if !gmod_bin.is_dir() && std::fs::create_dir_all(gmod_bin.as_os_str()).is_err() {return Err("failed to create garrysmod/lua/bin/goqui directory")}
		if let Ok(model_directory) = gmod_bin.read_dir() {return Ok(model_directory)}
		
		//or spit out that ambiguous error
		Err("failed to access garrysmod/lua/bin/goqui directory")
	}();
	
	//give up if we couldn't get the directory iterator
	if let Err(error) = directory_iterator {
		println!("[Goqui] Model loading failed with error: {error}");
		
		return table
	}
	
	for file in directory_iterator.unwrap().flatten() {
		let file_path = file.path();
		
		if file_path.is_dir() {
			let Ok(name) = file.file_name().into_string() else {continue};
			let model_struct = load_model(file_path);
			
			if let Err(error) = model_struct {
				println!("[Goqui] Failed to load {name} model with error: {error}");
				
				continue
			}
			
			let mut model_struct = model_struct.unwrap();
			model_struct.name = name.clone();
			
			table.insert(name, model_struct);
		}
	}
	
	println!("[Goqui] Models loaded.");
	
	table
});

//functions
unsafe fn add_module_function(lua: GLuaState, name: LuaString, func: LuaFunction) {
	lua.push_function(func);
	lua.set_field(-2, name);
}

/* source: https://github.com/Meachamp/voice-relay/blob/6164e90992b2435e17f9f8648317c2a0a6e3e821/server.js#L50-L101
let decodeOpusFrames = (buf, encoderState, id64) => {
	const maxRead = buf.length
	let readPos = 0
	let frames = []

	let readable = encoderState.stream
	let encoder = encoderState.encoder

	while(readPos < maxRead - 4) {
		let len = buf.readUInt16LE(readPos)
		readPos += 2

		let seq = buf.readUInt16LE(readPos)
		readPos += 2

		if(!encoderState.seq) {
			encoderState.seq = seq
		}

		if(seq < encoderState.seq) {
			encoderState.encoder = getEncoder()
			encoderState.seq = 0
		}
		else if(encoderState.seq != seq) {
			encoderState.seq = seq

			let lostFrames = Math.min(seq - encoderState.seq, 16)

			for(let i = 0; i < lostFrames; i++) {
				frames.push(encoder.decodePacketloss())
			}
		}

		encoderState.seq++;

		if(len <= 0 || seq < 0 || readPos + len > maxRead) {
			console.log(`Invalid packet LEN: ${len}, SEQ: ${seq}`)
			fs.writeFileSync('pckt_corr.dat', buf)
			return
		}

		const data = buf.slice(readPos, readPos + len)
		readPos += len

		let decodedFrame = encoder.decode(data)

		frames.push(decodedFrame)
	}

	let decompressedData = Buffer.concat(frames)
	readable.push(decompressedData)
}
*/

/*
	counter += 1;
											
	let mut wav_writer = audrey::hound::WavWriter::create(format!("{DATA_DIRECTORY}voice_{id}_{counter}.wav"),
		audrey::hound::WavSpec {
			bits_per_sample: 16,
			channels: 1,
			sample_format: audrey::hound::SampleFormat::Int,
			sample_rate: VOICE_SAMPLE_RATE,
		}
	).unwrap();

	for sample in output_buffer.iter() {wav_writer.write_sample(*sample).unwrap()}

	wav_writer.flush();
	wav_writer.finalize();
*/

unsafe fn listen_net<'a>(lua: GLuaState, host_address: &str, model_struct: &LoadedCoquiModel) -> E<'a> {
	let Ok(test) = UdpSocket::bind(host_address) else {return Err("failed to bind UDP socket")};
	let Ok(model_struct) = model_struct.clone() else {return Err("bruh")};
	
	if test.set_read_timeout(None).is_err() {return Err("failed to disable read timeout on UDP socket")};
	
	thread::spawn(move || {
		let mut counter: u32 = 0;
		let mut headed_buffer = [0u8; VOICE_HEADED_SIZE];
		let mut read_buffer = [0u8; VOICE_BUFFER_SIZE];
		//let mut sample_rate: u32 = 24000;
		let Ok(mut decoder) = opus::Decoder::new(VOICE_SAMPLE_RATE, opus::Channels::Mono) else {return};
		
		let mut voice_collection: VoiceBuilder = HashMap::new();
		
		loop {
			if let Ok(udp_length) = test.recv(&mut headed_buffer) {
			if let Ok(voice_queue) = VOICE_QUEUE.get_mut() {
				let mut cursor = std::io::Cursor::new(headed_buffer);
				let mut output_buffer = [0i16; VOICE_REPACKED_SIZE];
				let udp_length = udp_length as u64;
				let id = cursor.read_u64::<LittleEndian>().unwrap();
				
				while cursor.position() < udp_length {
					let code = cursor.read_u8().unwrap();
					let sixteen = cursor.read_u16::<LittleEndian>().unwrap();
					
					//println!("read u64 id {id} (i64 id {})\ncode is {code}\nsixteen is {sixteen}", id as i64);
					
					match code {
						//11 => println!("decoded opcode to SAMPLE_RATE"),
						
						6 => {
							//println!("decoded opcode to OPUSPLC");
							
							let goal = udp_length - 4;
							
							//shutup clippy
							#[allow(clippy::needless_range_loop)]
							while cursor.position() < goal {
								let length = cursor.read_u16::<LittleEndian>().unwrap() as usize;
								let read_iterator = read_buffer.iter_mut().enumerate();
								let march = cursor.read_u16::<LittleEndian>().unwrap();
								
								if length == 2 {continue} //skip if there's nothing to do
								if cursor.position() + length as u64 > goal {break} //dangerous
								
								for index in 0 .. length {read_buffer[index] = cursor.read_u8().unwrap()}
								for index in length .. VOICE_BUFFER_SIZE {read_buffer[index] = 0}
								
								let mut collection = voice_collection.get_mut(&id);
								
								if collection.is_none() {
									voice_collection.insert(id, vec![]);
									
									collection = voice_collection.get_mut(&id);
								}
								
								let collection = collection.unwrap();
								
								if collection.len() == VOICE_COLLECTION_THRESHOLD {
									let mut merger = [[0; VOICE_BUFFER_SIZE].as_slice(); VOICE_COLLECTION_THRESHOLD];
									let mut repacked = [0; VOICE_REPACKED_SIZE];
									let repacked_slice = repacked.as_mut_slice();
									let mut packetizer = opus::Repacketizer::new().unwrap();
									
									for (index, buffer) in collection.iter().enumerate() {merger[index] = buffer.as_slice();}
									
									if packetizer.combine(merger.as_mut_slice(), repacked_slice).is_ok() {
										match decoder.decode(repacked_slice, &mut output_buffer, false) {
											Ok(bruh) => {
												let Ok(model_struct) = model_struct.clone() else {continue};
												
												let mut wav_writer = audrey::hound::WavWriter::create(format!("{DATA_DIRECTORY}voice_{id}.wav"),
													audrey::hound::WavSpec {
														bits_per_sample: 8,
														channels: 1,
														sample_format: audrey::hound::SampleFormat::Int,
														sample_rate: VOICE_SAMPLE_RATE,
													}
												).unwrap();
												
												for sample in output_buffer.iter() {wav_writer.write_sample(*sample).unwrap();}
												
												if let Ok(text) = speech_to_text(model_struct, output_buffer.to_vec()) {voice_queue.push(text)}
											},
											
											Err(error) => {},//println!("opus error: {:?}", error),
										}
										
										//speech_to_text(model_struct, output);
										
										//voice_queue.push(value)
									}
								} else {collection.push(read_buffer)}
							}
						},
						
						//0 => println!("decoded opcode to SILENCE"),
						_ => {},
					}
				}
			}}
		}
	});
	
	Ok(())
}

unsafe fn pop_module_table(lua: GLuaState, table_name: LuaString) {lua.set_global(table_name)}

unsafe fn push_module_table(lua: GLuaState, table_name: LuaString) {
	lua.get_global(table_name);
	
	if lua.is_none_or_nil(-1) {
		lua.pop();
		lua.new_table();
	}
}

unsafe fn start_thinking(lua: GLuaState) {
	println!("[Goqui (Debug)] start_thinking");
	
	lua.get_global(lua_string!("timer"));
	lua.get_field(-1, lua_string!("Create"));
	lua.push_string("goqui");
	lua.push_integer(0);
	lua.push_integer(0);
	lua.push_function(lua_think);
	lua.call(4, 0);
	lua.pop();
}

unsafe fn stop_thinking(lua: GLuaState) {
	println!("[Goqui (Debug)] stop_thinking");
	
	lua.get_global(lua_string!("timer"));
	lua.get_field(-1, lua_string!("Remove"));
	lua.push_string("goqui");
	lua.call(1, 0);
	lua.pop();
}

fn speech_to_text(mut model_struct: LoadedCoquiModel, audio_buffer: Vec<i16>) -> Result<String, &'static str> {
	//reconstruct Result<String> into E
	match model_struct.model.speech_to_text(&audio_buffer) {
		Ok(text) => Ok(text),
		_ => Err("internal Coqui computation error"),
	}
}

fn prepare_file(model_struct: &LoadedCoquiModel, audio_path: String) -> Result<Vec<i16>, &'static str> {
	let Ok(audio_file) = File::open(audio_path) else {return Err("failed to open file")};
	let Ok(mut reader) = audrey::Reader::new(audio_file) else {return Err("failed to create audrey reader")};
	let description = reader.description();
	
	let channel_count = description.channel_count();
	let source_sample_rate = description.sample_rate();
	let target_sample_rate = model_struct.sample_rate;
	
	//make an audio buffer with the correct sample rate
	let mut audio_buffer: Vec<_> = if source_sample_rate == target_sample_rate {
		//samples() gives Result types which is stupid
		reader.samples().map(|sample| sample.unwrap()).collect()
	} else {
		dasp_signal::interpolate::Converter::from_hz_to_hz(
			dasp_signal::from_iter(reader.samples::<i16>().map(|sample| [sample.unwrap()])),
			dasp_interpolate::linear::Linear::new([0i16], [0]),
			source_sample_rate as f64,
			target_sample_rate as f64,
		).until_exhausted().map(|frame| frame[0]).collect()
	};
	
	//convert to mono
	if channel_count == 2 {audio_buffer = audio_buffer.chunks(2).map(|chunk| (chunk[0] + chunk[1]) / 2).collect()}
	else if channel_count != 1 {return Err("audio must be stereo or mono")}
	
	Ok(audio_buffer)
}

//lua functions
#[lua_function]
unsafe fn lua_compute(lua: GLuaState) -> i32 {
	let model_key = lua.check_string(1).to_string();
	let file_path = lua.check_string(2).to_string();
	
	lua.check_function(3);
	
	if lua.is_function(-2) {println!("-2 is function")}
	if lua.is_function(-1) {println!("-1 is function")}
	if lua.is_function(0) {println!("0 is function")}
	if lua.is_function(1) {println!("1 is function")}
	if lua.is_function(2) {println!("2 is function")}
	
	if let Some(model_struct) = MODEL_TABLE.get(&model_key) {
		let Ok(duplicated) = model_struct.clone() else {
			lua.push_boolean(false);
			lua.push_string("failed to duplicate model");
			
			return 2
		};
		
		thread::spawn(move || {
			let Ok(audio_buffer) = prepare_file(&duplicated, format!("{DATA_DIRECTORY}{file_path}")) else {return};
			let result = speech_to_text(duplicated, audio_buffer);
			
			match result {
				Err(error) => {},
				Ok(text) => {},
			}
		});
		
		lua.push_boolean(true);
		
		return 1
	}
	
	lua.push_boolean(false);
	lua.push_string("invalid model name");
	
	2
}

#[lua_function]
unsafe fn lua_get_model_details(lua: GLuaState) -> i32 {
	let model_key = lua.check_string(1).to_string();
	let Some(model_struct) = MODEL_TABLE.get(&model_key) else {return 0};
	
	lua.create_table(0, 3);
	
	lua.push_string(&model_struct.name);
	lua.set_field(-2, lua_string!("Name"));
	
	lua.push_number(model_struct.sample_rate as f64);
	lua.set_field(-2, lua_string!("SampleRate"));
	
	lua.push_boolean(model_struct.scorer_enabled);
	lua.set_field(-2, lua_string!("ScorerEnabled"));
	
	1
}

#[lua_function]
unsafe fn lua_get_models(lua: GLuaState) -> i32 {
	let mut counter = 0;
	
	lua.create_table(MODEL_TABLE.len() as i32, 0);
	
	for (key, _value) in MODEL_TABLE.iter() {
		counter += 1;
		
		lua.push_binary_string(key.as_bytes());
		lua.raw_seti(-2, counter);
	}
	
	1
}

#[lua_function]
unsafe fn lua_model_exists(lua: GLuaState) -> i32 {
	lua.push_boolean(MODEL_TABLE.contains_key(&lua.check_string(1).to_string()));
	
	1
}

#[lua_function]
unsafe fn lua_listen(lua: GLuaState) -> i32 {
	let model_key = lua.check_string(1).to_string();
	let host_address = lua.check_string(2).to_string();
	let host_port = lua.check_integer(3) as u16;
	let compounded_address = format!("{host_address}:{host_port}");
	
	let Some(model_struct) = MODEL_TABLE.get(&model_key) else {
		lua.push_boolean(false);
		lua.push_string("invalid model name");
		
		return 2
	};
	
	if let Err(error) = listen_net(lua, compounded_address.as_str(), model_struct) {
		lua.push_boolean(false);
		lua.push_string(error);
		
		return 2
	}
	
	lua.push_boolean(true);
	
	1
}

#[lua_function]
unsafe fn lua_think(lua: GLuaState) -> i32 {
	if let Ok(voice_heard) = VOICE_QUEUE.get_mut() {
		while let Some(text) = voice_heard.pop() {
			//lua_stack_guard!(lua => {
				lua.get_global(lua_string!("hook"));
				lua.get_field(-1, lua_string!("Run"));
				lua.push_string("GoquiEightBitHeard");
				lua.push_string(text.as_str());
				lua.call(2, 0);
				lua.pop();
			//});
		}
	}
	
	0
}

#[gmod13_open]
unsafe fn gmod13_open(lua: GLuaState) -> i32 {
	if lua.is_client() {override_stdout()}
	
	println!("[Goqui] Loading Coqui speech-to-text for Garry's Mod...");
	push_module_table(lua, lua_string!("goqui"));
		add_module_function(lua, lua_string!("Compute"), lua_compute);
		add_module_function(lua, lua_string!("GetModelDetails"), lua_get_model_details);
		add_module_function(lua, lua_string!("GetModels"), lua_get_models);
		add_module_function(lua, lua_string!("Listen8Bit"), lua_listen);
		add_module_function(lua, lua_string!("ModelExists"), lua_model_exists);
		add_module_function(lua, lua_string!("Think"), lua_think);
	pop_module_table(lua, lua_string!("goqui"));
	start_thinking(lua);
	println!("[Goqui] Done loading!");
	
	0
}

#[gmod13_close]
fn gmod13_close(_lua: GLuaState) -> i32 {0}