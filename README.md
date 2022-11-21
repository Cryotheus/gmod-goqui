# Goqui
**Very early in development!** (barely functional right now)  
This module is currently only in the proof-of-concept stage!  

Speech to text for Garry's Mod  
Does nothing on its own; this gives addon developers the ability to use Coqui Speech-To-Text in Garry's Mod.  
Written in Rust. Clients do not need it installed, but functionality is available client-side despite that.  

## Installation
1. Install [Coqui STT](https://github.com/coqui-ai/STT) and add `libstt.so.if.lib`/`libstt.so` to your machine's PATH environemnt variable.
2. Download a `.tflite` speech model and place it inside of its own folder in `GarrysMod/garrysmod/lua/bin/goqui`. You can download free compatible models from the [Coqui model archive](https://coqui.ai/models). The name of the folder is name of the model in Lua. The standard is to use ISO 639-1 codes (eg. `en-us` for English (United States) and `ru` for Russian)
3. Download the matching binary modules from [releases](https://github.com/Cryotheus/gmod-goqui/releases) (not yet available) and put it in `GarrysMod/garrysmod/lua/bin`.

## For Nerds
Everything below here is for Lua programmers.  
Don't create a timer named `goqui`, it will get overridden.
### Functions
All the functions listed below are available in the `goqui` table. Make sure `require("goqui")` is called once before using any of the functions. You don't need to head every file with it.
|  Key  | Arguments | Returns | Description |
| :---: | :-------: | :-----: | :---------- |
| Compute | `string: file` `string: model` `function(string: text): callback` | `bool: success` `string: error` | Given the file path (relative to the data folder) and a model name, calls the callback function with transcribed speech. The `error` return will be `nil` if the `success` return is `true` |
| Count | | `number` | Returns the amount of `Compute` calls that are still processing. |
| GetModelDetails | `string: model` | `table: details` | Returns a table of meta data about the stt model |
| GetModels | | `table: models` | Returns a sequential table of model names. This is the name of the folder containing the model files in the `GarrysMod/garrysmod/lua/bin/goqui` folder. |
| ModelExists | `string: model` | `bool` | Returns `true` if the model exists, or `false` if it doesn't. |
| Think | | | Internal function that is used to run the `callback` function argument given to `Compute`. Runs using a timer named `goqui`. |
