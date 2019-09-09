function isArabic(text) {
    var pattern = /[\u0600-\u06FF\u0750-\u077F]/;
    result = pattern.test(text);
    return result;
  }

  try {
module.exports = {
    isArabic : isArabic,
      generateTasks : function (links){
        let result = '';
        for (let index = 1; index <= links.length; index++) {
          result += `@task(${index})
          def f${index}(self):
           self.client.get("${links[index]}")
               
          `
        }
        return result;
      },
      generateFile :  function (tasks){
        return `
      from locust import HttpLocust, TaskSet, task
      
      
      class UserBehavior(TaskSet):
      
          def fon_start(self):
              self.login()
      
          def flogin(self):
              self.client.get("/")
      
          
          ${tasks}
      
      class WebsiteUser(HttpLocust):
          task_set = UserBehavior
          min_wait = 5000
          max_wait = 9000
      
        `
      }
}
}
catch (error){
console.log('We caught error');
};