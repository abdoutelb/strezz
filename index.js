const https = require('https');
const url = process.env.URL;
const fs = require('fs');

https.get(`https://api.hackertarget.com/pagelinks/?q=${url}`, (resp) => {
  let data = '';


  resp.on('data', (chunk) => {
    data += chunk;
  });

  resp.on('end', () => {
    
    let cleanUrls =  extractUrls(data)
    let tasks = generateTasks(cleanUrls);
    let pythonFile = generateFile(tasks);
    extractFile(pythonFile);
  });

}).on("error", (err) => {
  console.log("Error: " + err.message);
});


function extractUrls(urls){
    let cleanArray = urls.split("\n");
    let usedLinks = []
    for (let index = 0; index < cleanArray.length; index++) {
      if(cleanArray[index].startsWith(url))
      usedLinks.push(cleanArray[index].split(url)[1])
    }
    return usedLinks;
    } 
    
function generateTasks(links){
  let result = '';
  for (let index = 1; index <= links.length; index++) {
    result += `@task(${index})
    def f${index}(self):
     self.client.get("${links[index]}")
         
    `
  }
  return result;
};    

function generateFile(tasks){
  return `
  from locust import HttpLocust, TaskSet, task

class WebsiteTasks(TaskSet):
    def on_start(self):
    self.client.get("/")
    
    ${tasks}

class WebsiteUser(HttpLocust):
    task_set = WebsiteTasks
    min_wait = 5000
    max_wait = 15000
  `
}

function extractFile(python){
  
fs.writeFile("./locustfile.py", unicodeEscape(python),'utf8', function(err) {
    if(err) {
        return console.log(err);
    }
    console.log("done !");
}); 
}

function unicodeEscape(str) {
	return str.replace(/[\s\S]/g, function(character) {
		var escape = character.charCodeAt().toString(16),
		    longhand = escape.length > 2;
		return '\\' + (longhand ? 'u' : 'x') + ('0000' + escape).slice(longhand ? -4 : -2);
	});
}

