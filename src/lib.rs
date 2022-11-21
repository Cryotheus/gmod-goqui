#![feature(c_unwind, once_cell)]
//#![allow(dead_code, unused_assignments, unused_variables)] //debug only, will be removed in production

//test commands
//lua_run_cl require("goqui") PrintTable(goqui.GetModels())
//lua_run_cl print(goqui.Compute("goqui/gordead_ans18.wav", "en-us", function(...) print("callback!", ...) end))

use dasp_signal::Signal;
use gmod::gmcl::override_stdout;
use gmod::lua::{State as GLuaState, LuaFunction, LuaString};

use std::{
	cell::Cell,
	collections::HashMap,
	env::args,
	fs::{File, ReadDir},
	path::{Path, PathBuf},
	sync::LazyLock,
};

#[macro_use]
extern crate gmod;

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
static GMOD_EXE: LazyLock<String> = LazyLock::new(|| args().next().expect("Goqui Failed to identify gmod.exe"));

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
		let mut gmod_exe_path = PathBuf::from(GMOD_EXE.as_str());
		
		if gmod_exe_path.to_str().is_none() {return Err("Incompatible gmod.exe path")};
		while gmod_exe_path.file_name().unwrap() != "GarrysMod" && gmod_exe_path.pop() {} //ends_with was acting weird
		if !gmod_exe_path.ends_with("GarrysMod") {return Err("failed to find GarrysMod directory")}
		
		//one day, this won't be unsafe :D
		unsafe {DATA_DIRECTORY += &format!("{}/garrysmod/data/", gmod_exe_path.to_str().unwrap())}
		
		gmod_exe_path.push("garrysmod/lua/bin/goqui");
		
		//give up if the directory doesn't exist and we fail to create it
		if !gmod_exe_path.is_dir() && std::fs::create_dir_all(gmod_exe_path.as_os_str()).is_err() {return Err("failed to create garrysmod/lua/bin/goqui directory")}
		if let Ok(model_directory) = gmod_exe_path.read_dir() {return Ok(model_directory)}
		
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

thread_local! {static REMAINING: Cell<usize>  = Cell::new(0);}

//functions
unsafe fn add_module_function(lua: GLuaState, name: LuaString, func: LuaFunction) {
	lua.push_function(func);
	lua.set_field(-2, name);
}

fn decrement_remaining() -> usize {
	let mut new = 0usize;
	
	REMAINING.with(|remaining| {
		new = remaining.get() - 1;
		
		remaining.set(new);
	});
	
	new
}

fn get_remaining() -> usize {
	let mut output = 0usize;
	
	REMAINING.with(|remaining| output = remaining.get());
	
	output
}

fn increment_remaining() -> usize {
	let mut new = 0usize;
	
	REMAINING.with(|remaining| {
		new = remaining.get() + 1;
		
		remaining.set(new);
	});
	
	new
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
	lua.get_global(lua_string!("timer"));
	lua.get_field(-1, lua_string!("Remove"));
	lua.push_string("goqui");
	lua.call(1, 0);
	lua.pop();
}

fn speech_to_text(mut model_struct: LoadedCoquiModel, audio_path: String) -> Result<String, &'static str> {
	//lua.push_string(text);
	//lua.call(1, 0);
	
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
	
	//reconstruct Result<String> into Result<String, &'static str>
	match model_struct.model.speech_to_text(&audio_buffer) {
		Ok(text) => Ok(text),
		_ => Err("internal Coqui computation error"),
	}
}

//lua functions
#[lua_function]
unsafe fn lua_compute(lua: GLuaState) -> i32 {
	let file_path = lua.check_string(1).to_string();
	let model_key = lua.check_string(2).to_string();
	
	lua.check_function(3);
	
	if let Some(model_struct) = MODEL_TABLE.get(&model_key) {
		//let bruh = &model_struct.name;
		//lua.push_string(format!("nothing for now, but the {bruh} model would produce a string").as_str());
		//lua.call(1, 0);
		
		let Ok(duplicated) = model_struct.clone() else {
			lua.push_boolean(false);
			lua.push_string("failed to duplicate model");
			
			return 2
		};
		
		let result = speech_to_text(duplicated, format!("{DATA_DIRECTORY}{file_path}"));
		
		match result {
			Err(error) => {
				lua.push_boolean(false);
				lua.push_string(error);
				
				return 2
			},
			
			Ok(_) => {
				if increment_remaining() == 1 {start_thinking(lua)}
				
				lua.push_boolean(false);
				
				return 1
			},
		}
	}
	
	lua.push_boolean(false);
	lua.push_string("invalid model name");
	
	2
}

#[lua_function]
unsafe fn lua_count(lua: GLuaState) -> i32 {
	lua.push_number(get_remaining() as f64);
	
	1
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
unsafe fn lua_think(lua: GLuaState) -> i32 {
	//TODO: write lua_think function internals")
	
	if get_remaining() == 0 {stop_thinking(lua)}
	
	0
}

#[gmod13_open]
unsafe fn gmod13_open(lua: GLuaState) -> i32 {
	if lua.is_client() {override_stdout()}
	
	println!("[Goqui] Loading Coqui speech to text for Garry's Mod...");
	push_module_table(lua, lua_string!("goqui"));
		add_module_function(lua, lua_string!("Compute"), lua_compute);
		add_module_function(lua, lua_string!("Count"), lua_count);
		add_module_function(lua, lua_string!("GetModelDetails"), lua_get_model_details);
		add_module_function(lua, lua_string!("GetModels"), lua_get_models);
		add_module_function(lua, lua_string!("ModelExists"), lua_model_exists);
		add_module_function(lua, lua_string!("Think"), lua_think);
	pop_module_table(lua, lua_string!("goqui"));
	println!("[Goqui] Done loading!");
	
	0
}

#[gmod13_close]
fn gmod13_close(_lua: GLuaState) -> i32 {0}